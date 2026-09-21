use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, ffi::ErrorCode, params};

use crate::{
    Coordinates, EvidenceError, FeedbackEvent, FeedbackEventId, FeedbackKind,
    MAX_RELATION_NAME_BYTES, MemoryKind, MemoryNode, Metadata, NewFeedbackEvent, NewMemory,
    NewRelation, NodeId, Relation, ValidationError, normalize_terms, validate_string,
    validate_unit_interval,
};

const FORMAT_ID: &str = "memory3d";
const SCHEMA_VERSION: i64 = 7;
const BUSY_TIMEOUT: Duration = Duration::from_millis(250);

const CREATE_SCHEMA_V1: &str = r"
CREATE TABLE nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (id > 0),
    kind TEXT NOT NULL CHECK (length(kind) > 0 AND length(CAST(kind AS BLOB)) <= 64),
    text TEXT NOT NULL CHECK (length(trim(text)) > 0 AND length(CAST(text AS BLOB)) <= 1048576),
    metadata TEXT NOT NULL CHECK (
        json_valid(metadata) AND json_type(metadata) = 'object'
        AND length(CAST(metadata AS BLOB)) <= 65536
    ),
    importance REAL NOT NULL CHECK (importance >= 0.0 AND importance <= 1.0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    x REAL,
    y REAL,
    z REAL,
    CHECK ((x IS NULL AND y IS NULL AND z IS NULL) OR
           (x IS NOT NULL AND y IS NOT NULL AND z IS NOT NULL)),
    CHECK (x IS NULL OR (typeof(x) = 'real' AND x >= -1000000.0 AND x <= 1000000.0)),
    CHECK (y IS NULL OR (typeof(y) = 'real' AND y >= -1000000.0 AND y <= 1000000.0)),
    CHECK (z IS NULL OR (typeof(z) = 'real' AND z >= -1000000.0 AND z <= 1000000.0))
);

CREATE TABLE relations (
    source_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE RESTRICT,
    target_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (
        length(trim(name)) > 0 AND length(CAST(name AS BLOB)) <= 64
    ),
    weight REAL NOT NULL CHECK (weight >= 0.0 AND weight <= 1.0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    reinforcement_count INTEGER NOT NULL DEFAULT 0 CHECK (reinforcement_count >= 0),
    PRIMARY KEY (source_id, target_id, name)
);

CREATE INDEX relations_source_idx ON relations(source_id, target_id, name);
CREATE INDEX relations_target_idx ON relations(target_id, source_id, name);
";

const CREATE_SCHEMA_V2: &str = r"
CREATE TABLE node_terms (
    node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    term TEXT NOT NULL CHECK (length(term) > 0),
    PRIMARY KEY (node_id, term)
);

CREATE INDEX node_terms_term_idx ON node_terms(term, node_id);
";

const CREATE_SCHEMA_V3: &str = r"
CREATE INDEX relations_adjacency_idx ON relations(source_id, weight DESC);
";

const CREATE_SCHEMA_V4: &str = r"
CREATE TABLE relation_feedback_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (id > 0),
    source_id INTEGER NOT NULL,
    target_id INTEGER NOT NULL,
    relation_name TEXT NOT NULL CHECK (
        length(trim(relation_name)) > 0 AND length(CAST(relation_name AS BLOB)) <= 64
    ),
    kind TEXT NOT NULL CHECK (kind IN ('positive', 'negative')),
    strength REAL NOT NULL CHECK (strength > 0.0 AND strength <= 1.0),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    FOREIGN KEY (source_id, target_id, relation_name)
        REFERENCES relations(source_id, target_id, name) ON DELETE RESTRICT
);

CREATE INDEX relation_feedback_relation_idx
    ON relation_feedback_events(source_id, target_id, relation_name, occurred_at_ms, id);
";

const CREATE_SCHEMA_V5: &str = r"
CREATE TABLE community_graph_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    graph_revision INTEGER NOT NULL CHECK (graph_revision >= 0)
);
INSERT INTO community_graph_state (singleton, graph_revision) VALUES (1, 0);

CREATE TABLE community_builds (
    policy TEXT PRIMARY KEY,
    graph_revision INTEGER NOT NULL CHECK (graph_revision >= 0),
    built_at_ms INTEGER NOT NULL CHECK (built_at_ms >= 0),
    fine_threshold REAL NOT NULL CHECK (fine_threshold >= 0.0 AND fine_threshold <= 1.0),
    coarse_threshold REAL NOT NULL CHECK (coarse_threshold >= 0.0 AND coarse_threshold <= 1.0),
    build_node_work INTEGER NOT NULL CHECK (build_node_work >= 0),
    build_edge_work INTEGER NOT NULL CHECK (build_edge_work >= 0)
);

CREATE TABLE community_memberships (
    policy TEXT NOT NULL REFERENCES community_builds(policy) ON DELETE CASCADE,
    resolution INTEGER NOT NULL CHECK (resolution IN (0, 1)),
    community_id INTEGER NOT NULL CHECK (community_id > 0),
    node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    PRIMARY KEY (policy, resolution, node_id)
);
CREATE INDEX community_memberships_lookup_idx
    ON community_memberships(policy, resolution, community_id, node_id);

CREATE TRIGGER community_nodes_insert_revision
AFTER INSERT ON nodes BEGIN
    UPDATE community_graph_state SET graph_revision = graph_revision + 1 WHERE singleton = 1;
END;
CREATE TRIGGER community_nodes_update_revision
AFTER UPDATE ON nodes BEGIN
    UPDATE community_graph_state SET graph_revision = graph_revision + 1 WHERE singleton = 1;
END;
CREATE TRIGGER community_nodes_delete_revision
AFTER DELETE ON nodes BEGIN
    UPDATE community_graph_state SET graph_revision = graph_revision + 1 WHERE singleton = 1;
END;
CREATE TRIGGER community_relations_insert_revision
AFTER INSERT ON relations BEGIN
    UPDATE community_graph_state SET graph_revision = graph_revision + 1 WHERE singleton = 1;
END;
CREATE TRIGGER community_relations_update_revision
AFTER UPDATE OF source_id, target_id, name, weight ON relations BEGIN
    UPDATE community_graph_state SET graph_revision = graph_revision + 1 WHERE singleton = 1;
END;
CREATE TRIGGER community_relations_delete_revision
AFTER DELETE ON relations BEGIN
    UPDATE community_graph_state SET graph_revision = graph_revision + 1 WHERE singleton = 1;
END;
";

const CREATE_SCHEMA_V6: &str = r"
CREATE TABLE evidence_bundles (
    id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (id > 0),
    idempotency_key TEXT NOT NULL UNIQUE CHECK (
        length(trim(idempotency_key)) > 0
        AND length(CAST(idempotency_key AS BLOB)) <= 128
    ),
    canonical_payload TEXT NOT NULL CHECK (
        json_valid(canonical_payload)
        AND length(CAST(canonical_payload AS BLOB)) <= 4194304
    ),
    applied_by TEXT NOT NULL CHECK (
        length(trim(applied_by)) > 0 AND length(CAST(applied_by AS BLOB)) <= 256
    ),
    apply_policy TEXT NOT NULL CHECK (
        length(trim(apply_policy)) > 0 AND length(CAST(apply_policy AS BLOB)) <= 256
    ),
    applied_at_ms INTEGER NOT NULL CHECK (applied_at_ms >= 0),
    rolled_back_at_ms INTEGER CHECK (rolled_back_at_ms IS NULL OR rolled_back_at_ms >= applied_at_ms)
);

CREATE TABLE evidence_items (
    evidence_id TEXT PRIMARY KEY CHECK (
        length(trim(evidence_id)) > 0 AND length(CAST(evidence_id AS BLOB)) <= 128
    ),
    bundle_id INTEGER NOT NULL REFERENCES evidence_bundles(id) ON DELETE RESTRICT,
    node_id INTEGER NOT NULL UNIQUE REFERENCES nodes(id) ON DELETE RESTRICT,
    source_identity TEXT NOT NULL CHECK (
        length(trim(source_identity)) > 0 AND length(CAST(source_identity AS BLOB)) <= 256
    ),
    producer_identity TEXT NOT NULL CHECK (
        length(trim(producer_identity)) > 0 AND length(CAST(producer_identity AS BLOB)) <= 256
    ),
    scope_json TEXT NOT NULL CHECK (json_valid(scope_json) AND json_type(scope_json) = 'object'),
    lifecycle TEXT NOT NULL CHECK (lifecycle IN (
        'candidate', 'observed', 'corroborated', 'approved',
        'contradicted', 'superseded', 'rejected'
    )),
    evidence_created_at_ms INTEGER NOT NULL CHECK (evidence_created_at_ms >= 0),
    observed_at_ms INTEGER CHECK (observed_at_ms IS NULL OR observed_at_ms >= 0),
    conflict_group TEXT CHECK (
        conflict_group IS NULL OR (
            length(trim(conflict_group)) > 0
            AND length(CAST(conflict_group AS BLOB)) <= 128
        )
    ),
    decision_actor TEXT,
    decision_policy TEXT,
    decided_at_ms INTEGER,
    CHECK (
        (decision_actor IS NULL AND decision_policy IS NULL AND decided_at_ms IS NULL)
        OR (decision_actor IS NOT NULL AND decision_policy IS NOT NULL AND decided_at_ms >= 0)
    )
);
CREATE INDEX evidence_items_bundle_idx ON evidence_items(bundle_id, evidence_id);
CREATE INDEX evidence_items_node_idx ON evidence_items(node_id, evidence_id);
CREATE INDEX evidence_items_conflict_idx ON evidence_items(conflict_group, evidence_id);

CREATE TABLE evidence_derivations (
    evidence_id TEXT NOT NULL REFERENCES evidence_items(evidence_id) ON DELETE RESTRICT,
    supporting_evidence_id TEXT NOT NULL REFERENCES evidence_items(evidence_id) ON DELETE RESTRICT,
    PRIMARY KEY (evidence_id, supporting_evidence_id),
    CHECK (evidence_id <> supporting_evidence_id)
);
CREATE INDEX evidence_derivations_support_idx
    ON evidence_derivations(supporting_evidence_id, evidence_id);

CREATE TABLE evidence_conflicts (
    evidence_id TEXT NOT NULL REFERENCES evidence_items(evidence_id) ON DELETE RESTRICT,
    conflicting_evidence_id TEXT NOT NULL REFERENCES evidence_items(evidence_id) ON DELETE RESTRICT,
    PRIMARY KEY (evidence_id, conflicting_evidence_id),
    CHECK (evidence_id <> conflicting_evidence_id)
);
CREATE INDEX evidence_conflicts_target_idx
    ON evidence_conflicts(conflicting_evidence_id, evidence_id);

CREATE TABLE evidence_supersessions (
    evidence_id TEXT NOT NULL REFERENCES evidence_items(evidence_id) ON DELETE RESTRICT,
    superseded_evidence_id TEXT NOT NULL REFERENCES evidence_items(evidence_id) ON DELETE RESTRICT,
    PRIMARY KEY (evidence_id, superseded_evidence_id),
    CHECK (evidence_id <> superseded_evidence_id)
);
CREATE INDEX evidence_supersessions_target_idx
    ON evidence_supersessions(superseded_evidence_id, evidence_id);

CREATE TABLE evidence_decision_support (
    evidence_id TEXT NOT NULL REFERENCES evidence_items(evidence_id) ON DELETE RESTRICT,
    supporting_evidence_id TEXT NOT NULL REFERENCES evidence_items(evidence_id) ON DELETE RESTRICT,
    PRIMARY KEY (evidence_id, supporting_evidence_id),
    CHECK (evidence_id <> supporting_evidence_id)
);
CREATE INDEX evidence_decision_support_target_idx
    ON evidence_decision_support(supporting_evidence_id, evidence_id);
";

/// Failures returned by durable repository operations.
#[derive(Debug)]
pub enum RepositoryError {
    /// A public domain value failed validation.
    Validation(ValidationError),
    /// `SQLite` rejected or could not complete an operation.
    Database(rusqlite::Error),
    /// Another connection kept the database busy or locked past the bounded wait.
    Busy,
    /// The database is not a recognized `Memory3D` database.
    InvalidDatabase(String),
    /// The database was created by a newer unsupported schema.
    UnsupportedSchema {
        /// Version stored in the database.
        found: i64,
        /// Newest version understood by this library.
        supported: i64,
    },
    /// A referenced node does not exist.
    NodeNotFound(NodeId),
    /// A requested relation does not exist.
    RelationNotFound {
        /// Relation source node.
        source: NodeId,
        /// Relation target node.
        target: NodeId,
        /// Relation name.
        name: String,
    },
    /// A requested feedback event does not exist.
    FeedbackEventNotFound(FeedbackEventId),
    /// A versioned evidence DTO failed structural validation.
    Evidence(EvidenceError),
    /// An immutable evidence ID already exists in an active bundle.
    EvidenceIdExists(String),
    /// An evidence reference does not resolve to an active stored or same-bundle item.
    EvidenceReferenceNotFound {
        /// Evidence item declaring the reference.
        evidence_id: String,
        /// Reference category.
        relation: &'static str,
        /// Missing target evidence ID.
        target: String,
    },
    /// Conflict members do not declare the same stable conflict group.
    EvidenceConflictGroupMismatch {
        /// First evidence ID.
        evidence_id: String,
        /// Referenced competing evidence ID.
        target: String,
    },
    /// An idempotency key was reused for a different canonical payload.
    IdempotencyConflict(String),
    /// A prior application with this key was explicitly rolled back.
    EvidenceBundleRolledBack(String),
    /// No evidence-bundle ledger entry exists for the requested key.
    EvidenceBundleNotFound(String),
    /// A later durable record prevents compensating deletion of a bundle.
    EvidenceRollbackBlocked {
        /// Bundle key requested for rollback.
        idempotency_key: String,
        /// Later evidence ID or external graph relation that depends on it.
        referenced_by: String,
    },
    /// No persisted community artifact exists for the requested policy.
    CommunityArtifactsMissing,
    /// Persisted communities were built from an older raw graph revision.
    CommunityArtifactsStale {
        /// Revision from which the artifacts were built.
        built_revision: u64,
        /// Current raw graph revision.
        current_revision: u64,
    },
    /// The system clock could not produce a durable timestamp.
    Clock(SystemTimeError),
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => error.fmt(formatter),
            Self::Database(error) => write!(formatter, "database operation failed: {error}"),
            Self::Busy => formatter.write_str(
                "database is busy or locked by another writer; retry after that writer finishes",
            ),
            Self::InvalidDatabase(reason) => {
                write!(formatter, "invalid Memory3D database: {reason}")
            }
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "database schema version {found} is newer than supported version {supported}"
            ),
            Self::NodeNotFound(id) => write!(formatter, "node {id} does not exist"),
            Self::RelationNotFound {
                source,
                target,
                name,
            } => {
                write!(
                    formatter,
                    "relation {source} -[{name}]-> {target} does not exist"
                )
            }
            Self::FeedbackEventNotFound(id) => {
                write!(formatter, "feedback event {id} does not exist")
            }
            Self::Evidence(error) => error.fmt(formatter),
            Self::EvidenceIdExists(id) => {
                write!(formatter, "evidence ID {id:?} already exists")
            }
            Self::EvidenceReferenceNotFound {
                evidence_id,
                relation,
                target,
            } => write!(
                formatter,
                "evidence {evidence_id:?} has missing {relation} reference {target:?}"
            ),
            Self::EvidenceConflictGroupMismatch {
                evidence_id,
                target,
            } => write!(
                formatter,
                "conflicting evidence {evidence_id:?} and {target:?} must share one conflict group"
            ),
            Self::IdempotencyConflict(key) => write!(
                formatter,
                "idempotency key {key:?} already identifies a different evidence payload"
            ),
            Self::EvidenceBundleRolledBack(key) => write!(
                formatter,
                "evidence bundle {key:?} was rolled back and cannot be silently reapplied"
            ),
            Self::EvidenceBundleNotFound(key) => {
                write!(formatter, "evidence bundle {key:?} does not exist")
            }
            Self::EvidenceRollbackBlocked {
                idempotency_key,
                referenced_by,
            } => write!(
                formatter,
                "evidence bundle {idempotency_key:?} cannot be rolled back because {referenced_by} depends on it"
            ),
            Self::CommunityArtifactsMissing => {
                formatter.write_str("community artifacts are missing; build them before activation")
            }
            Self::CommunityArtifactsStale {
                built_revision,
                current_revision,
            } => write!(
                formatter,
                "community artifacts are stale (built at revision {built_revision}, current revision {current_revision}); rebuild before activation"
            ),
            Self::Clock(error) => write!(formatter, "system clock error: {error}"),
        }
    }
}

impl Error for RepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Evidence(error) => Some(error),
            Self::Database(error) => Some(error),
            Self::Clock(error) => Some(error),
            Self::Busy
            | Self::InvalidDatabase(_)
            | Self::UnsupportedSchema { .. }
            | Self::NodeNotFound(_)
            | Self::RelationNotFound { .. }
            | Self::FeedbackEventNotFound(_)
            | Self::EvidenceIdExists(_)
            | Self::EvidenceReferenceNotFound { .. }
            | Self::EvidenceConflictGroupMismatch { .. }
            | Self::IdempotencyConflict(_)
            | Self::EvidenceBundleRolledBack(_)
            | Self::EvidenceBundleNotFound(_)
            | Self::EvidenceRollbackBlocked { .. }
            | Self::CommunityArtifactsMissing
            | Self::CommunityArtifactsStale { .. } => None,
        }
    }
}

impl From<ValidationError> for RepositoryError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}

impl From<EvidenceError> for RepositoryError {
    fn from(value: EvidenceError) -> Self {
        Self::Evidence(value)
    }
}

impl From<rusqlite::Error> for RepositoryError {
    fn from(value: rusqlite::Error) -> Self {
        match value.sqlite_error_code() {
            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => Self::Busy,
            _ => Self::Database(value),
        }
    }
}

/// Result of `SQLite` and application-level checks performed without modifying the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityReport {
    messages: Vec<String>,
}

impl IntegrityReport {
    /// Returns true when `SQLite` reported no structural or foreign-key problems.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.messages.is_empty()
    }

    /// Returns diagnostic messages reported by `SQLite`.
    #[must_use]
    pub fn messages(&self) -> &[String] {
        &self.messages
    }
}

impl From<SystemTimeError> for RepositoryError {
    fn from(value: SystemTimeError) -> Self {
        Self::Clock(value)
    }
}

/// Synchronous repository backed by one versioned `SQLite` file.
pub struct Repository {
    pub(crate) connection: Connection,
    path: PathBuf,
}

impl Repository {
    /// Creates or opens a `Memory3D` database and applies supported migrations transactionally.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid path, unrecognized database, `SQLite` failure, or schema
    /// newer than this library supports.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RepositoryError> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return Err(RepositoryError::InvalidDatabase("path is empty".to_owned()));
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "foreign_keys", false)?;
        let mut repository = Self {
            connection,
            path: path.to_path_buf(),
        };
        repository.open_or_migrate()?;
        repository
            .connection
            .pragma_update(None, "foreign_keys", true)?;
        Ok(repository)
    }

    /// Returns the database file path supplied at open time.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Checks an existing database through a read-only connection.
    ///
    /// This runs `SQLite`'s full integrity check and foreign-key check, then verifies the
    /// `Memory3D` format identifier and supported schema version. It never creates or migrates a
    /// database.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read as a recognized, supported `Memory3D`
    /// database. Structural problems that `SQLite` can enumerate are returned in the report.
    pub fn check(path: impl AsRef<Path>) -> Result<IntegrityReport, RepositoryError> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return Err(RepositoryError::InvalidDatabase("path is empty".to_owned()));
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;

        let mut messages = Vec::new();
        {
            let mut statement = connection.prepare("PRAGMA integrity_check")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                let message = row?;
                if message != "ok" {
                    messages.push(message);
                }
            }
        }
        {
            let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
            let rows = statement.query_map([], |row| {
                Ok(format!(
                    "foreign key violation: table={}, rowid={}, parent={}, constraint={}",
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?
                        .map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?
                ))
            })?;
            messages.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        let nodes_table_exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'nodes')",
            [],
            |row| row.get(0),
        )?;
        if nodes_table_exists {
            let mut statement = connection.prepare(
                "SELECT id FROM nodes
                 WHERE (x IS NULL) <> (y IS NULL)
                    OR (x IS NULL) <> (z IS NULL)
                    OR (x IS NOT NULL AND (typeof(x) <> 'real' OR x < -1000000.0 OR x > 1000000.0))
                    OR (y IS NOT NULL AND (typeof(y) <> 'real' OR y < -1000000.0 OR y > 1000000.0))
                    OR (z IS NOT NULL AND (typeof(z) <> 'real' OR z < -1000000.0 OR z > 1000000.0))
                 ORDER BY id",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
            for id in rows {
                messages.push(format!("node {} has invalid coordinates", id?));
            }
        }

        let metadata = connection
            .query_row(
                "SELECT format_id, schema_version FROM memory3d_schema WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|error| {
                RepositoryError::InvalidDatabase(format!("schema metadata is unreadable: {error}"))
            })?
            .ok_or_else(|| {
                RepositoryError::InvalidDatabase("schema metadata is missing".to_owned())
            })?;
        if metadata.0 != FORMAT_ID {
            return Err(RepositoryError::InvalidDatabase(format!(
                "format identifier is {:?}",
                metadata.0
            )));
        }
        if metadata.1 > SCHEMA_VERSION {
            return Err(RepositoryError::UnsupportedSchema {
                found: metadata.1,
                supported: SCHEMA_VERSION,
            });
        }
        if !(0..=SCHEMA_VERSION).contains(&metadata.1) {
            return Err(RepositoryError::InvalidDatabase(format!(
                "unsupported historical schema version {}",
                metadata.1
            )));
        }
        Ok(IntegrityReport { messages })
    }

    /// Adds one node and returns its complete durable representation.
    ///
    /// # Errors
    ///
    /// Returns a repository error when serialization, timekeeping, or the database write fails.
    pub fn add_node(&mut self, node: NewMemory) -> Result<MemoryNode, RepositoryError> {
        let mut nodes = self.add_nodes(&[node])?;
        nodes.pop().ok_or_else(|| {
            RepositoryError::InvalidDatabase("node insert returned no record".to_owned())
        })
    }

    /// Adds all nodes atomically in input order.
    ///
    /// # Errors
    ///
    /// Returns a repository error and rolls back every insert if any write fails.
    pub fn add_nodes(&mut self, nodes: &[NewMemory]) -> Result<Vec<MemoryNode>, RepositoryError> {
        let now = unix_time_ms()?;
        let transaction = self.connection.transaction()?;
        let mut stored = Vec::with_capacity(nodes.len());
        for node in nodes {
            let metadata = node.metadata.to_json().map_err(|error| {
                RepositoryError::InvalidDatabase(format!("metadata serialization failed: {error}"))
            })?;
            let (x, y, z) = coordinates_parts(node.coordinates);
            transaction.execute(
                "INSERT INTO nodes (kind, text, metadata, importance, created_at_ms, updated_at_ms, x, y, z) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8)",
                params![node.kind.as_str(), node.text, metadata, node.importance, now, x, y, z],
            )?;
            let id = id_from_i64(transaction.last_insert_rowid())?;
            index_node_terms(&transaction, id, node.kind.as_str(), &node.text)?;
            stored.push(MemoryNode::from_stored(id, node.clone(), now, now));
        }
        transaction.commit()?;
        Ok(stored)
    }

    /// Gets a node by stable ID.
    ///
    /// # Errors
    ///
    /// Returns a repository error for malformed persisted data or a database failure.
    pub fn get_node(&self, id: NodeId) -> Result<Option<MemoryNode>, RepositoryError> {
        let raw = self
            .connection
            .query_row(
                "SELECT id, kind, text, metadata, importance, created_at_ms, updated_at_ms, x, y, z FROM nodes WHERE id = ?1",
                [id_to_i64(id)?],
                read_node_row,
            )
            .optional()?;
        raw.map(decode_node).transpose()
    }

    /// Replaces or clears one node's externally assigned coordinates.
    ///
    /// This updates the node timestamp transactionally.
    ///
    /// # Errors
    ///
    /// Returns a typed missing-node error or a database/decoding error.
    pub fn update_node_coordinates(
        &mut self,
        id: NodeId,
        coordinates: Option<Coordinates>,
    ) -> Result<MemoryNode, RepositoryError> {
        let now = unix_time_ms()?;
        let (x, y, z) = coordinates_parts(coordinates);
        let changed = self.connection.execute(
            "UPDATE nodes
             SET x = ?2, y = ?3, z = ?4,
                 updated_at_ms = CASE
                     WHEN updated_at_ms >= ?5 THEN updated_at_ms + 1
                     ELSE ?5
                 END
             WHERE id = ?1",
            params![id_to_i64(id)?, x, y, z, now],
        )?;
        if changed == 0 {
            return Err(RepositoryError::NodeNotFound(id));
        }
        self.get_node(id)?.ok_or_else(|| {
            RepositoryError::InvalidDatabase(format!(
                "node {id} disappeared after coordinate update"
            ))
        })
    }

    /// Lists all nodes in stable ID order.
    ///
    /// # Errors
    ///
    /// Returns a repository error for malformed persisted data or a database failure.
    pub fn list_nodes(&self) -> Result<Vec<MemoryNode>, RepositoryError> {
        let mut statement = self.connection.prepare(
            "SELECT id, kind, text, metadata, importance, created_at_ms, updated_at_ms, x, y, z FROM nodes ORDER BY id",
        )?;
        let rows = statement.query_map([], read_node_row)?;
        rows.map(|row| decode_node(row?)).collect()
    }

    /// Adds one directed relation.
    ///
    /// # Errors
    ///
    /// Returns a typed missing-node error or a database error.
    pub fn add_relation(&mut self, relation: NewRelation) -> Result<Relation, RepositoryError> {
        let mut relations = self.add_relations(&[relation])?;
        relations.pop().ok_or_else(|| {
            RepositoryError::InvalidDatabase("relation insert returned no record".to_owned())
        })
    }

    /// Adds all directed relations atomically in input order.
    ///
    /// # Errors
    ///
    /// Returns an error and rolls back the batch if a node is missing, a relation conflicts, or
    /// any database write fails.
    pub fn add_relations(
        &mut self,
        relations: &[NewRelation],
    ) -> Result<Vec<Relation>, RepositoryError> {
        let now = unix_time_ms()?;
        let transaction = self.connection.transaction()?;
        let mut stored = Vec::with_capacity(relations.len());
        for relation in relations {
            ensure_node_exists(&transaction, relation.source)?;
            ensure_node_exists(&transaction, relation.target)?;
            transaction.execute(
                "INSERT INTO relations (source_id, target_id, name, weight, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![id_to_i64(relation.source)?, id_to_i64(relation.target)?, relation.name, relation.weight, now],
            )?;
            stored.push(Relation::from_stored(relation.clone(), now, now, 0));
        }
        transaction.commit()?;
        Ok(stored)
    }

    /// Gets one directed relation by its composite identity.
    ///
    /// # Errors
    ///
    /// Returns a validation, decoding, or database error.
    pub fn get_relation(
        &self,
        source: NodeId,
        target: NodeId,
        name: &str,
    ) -> Result<Option<Relation>, RepositoryError> {
        let name = validate_string(name.to_owned(), "relation name", MAX_RELATION_NAME_BYTES)?;
        let raw = self
            .connection
            .query_row(
                "SELECT source_id, target_id, name, weight, created_at_ms, updated_at_ms, reinforcement_count FROM relations WHERE source_id = ?1 AND target_id = ?2 AND name = ?3",
                params![id_to_i64(source)?, id_to_i64(target)?, name],
                read_relation_row,
            )
            .optional()?;
        raw.map(decode_relation).transpose()
    }

    /// Lists directed relations in deterministic composite-key order.
    ///
    /// # Errors
    ///
    /// Returns a repository error for malformed persisted data or a database failure.
    pub fn list_relations(&self) -> Result<Vec<Relation>, RepositoryError> {
        let mut statement = self.connection.prepare(
            "SELECT source_id, target_id, name, weight, created_at_ms, updated_at_ms, reinforcement_count FROM relations ORDER BY source_id, target_id, name",
        )?;
        let rows = statement.query_map([], read_relation_row)?;
        rows.map(|row| decode_relation(row?)).collect()
    }

    /// Updates a relation weight and returns the updated durable relation.
    ///
    /// # Errors
    ///
    /// Returns a validation error, [`RepositoryError::RelationNotFound`], or a database error.
    pub fn update_relation_weight(
        &mut self,
        source: NodeId,
        target: NodeId,
        name: &str,
        weight: f32,
    ) -> Result<Relation, RepositoryError> {
        validate_unit_interval(weight, "weight")?;
        let name = validate_string(name.to_owned(), "relation name", MAX_RELATION_NAME_BYTES)?;
        let now = unix_time_ms()?;
        let changed = self.connection.execute(
            "UPDATE relations SET weight = ?1, updated_at_ms = max(updated_at_ms, ?2) WHERE source_id = ?3 AND target_id = ?4 AND name = ?5",
            params![weight, now, id_to_i64(source)?, id_to_i64(target)?, name],
        )?;
        if changed == 0 {
            return Err(RepositoryError::RelationNotFound {
                source,
                target,
                name,
            });
        }
        self.get_relation(source, target, &name)?.ok_or_else(|| {
            RepositoryError::InvalidDatabase("updated relation disappeared".to_owned())
        })
    }

    /// Records one explicit feedback event for an existing relation.
    ///
    /// Positive events increment the relation's informational `reinforcement_count`; negative
    /// events are stored separately and never create invalid weights. Activation is affected only
    /// by opt-in feedback-aware retrieval.
    ///
    /// # Errors
    ///
    /// Returns a validation error, [`RepositoryError::RelationNotFound`], or a database error.
    pub fn record_relation_feedback(
        &mut self,
        event: NewFeedbackEvent,
    ) -> Result<FeedbackEvent, RepositoryError> {
        let now = unix_time_ms()?;
        let transaction = self.connection.transaction()?;
        ensure_relation_exists(
            &transaction,
            event.source,
            event.target,
            &event.relation_name,
        )?;
        transaction.execute(
            "INSERT INTO relation_feedback_events (source_id, target_id, relation_name, kind, strength, occurred_at_ms, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id_to_i64(event.source)?,
                id_to_i64(event.target)?,
                event.relation_name,
                event.kind.as_str(),
                event.strength,
                event.occurred_at_ms,
                now
            ],
        )?;
        let id = feedback_id_from_i64(transaction.last_insert_rowid())?;
        let increment = i64::from(matches!(event.kind, FeedbackKind::Positive));
        transaction.execute(
            "UPDATE relations SET reinforcement_count = reinforcement_count + ?1, updated_at_ms = max(updated_at_ms, ?2) WHERE source_id = ?3 AND target_id = ?4 AND name = ?5",
            params![
                increment,
                now,
                id_to_i64(event.source)?,
                id_to_i64(event.target)?,
                event.relation_name
            ],
        )?;
        transaction.commit()?;
        Ok(FeedbackEvent::from_stored(id, event, now))
    }

    /// Removes one feedback event and returns the removed event.
    ///
    /// This is the rollback operation for feedback-aware retrieval. It only removes the selected
    /// event and compensates the informational positive counter when needed.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::FeedbackEventNotFound`] or a database error.
    pub fn remove_feedback_event(
        &mut self,
        id: FeedbackEventId,
    ) -> Result<FeedbackEvent, RepositoryError> {
        let transaction = self.connection.transaction()?;
        let event = get_feedback_event_in_transaction(&transaction, id)?
            .ok_or(RepositoryError::FeedbackEventNotFound(id))?;
        transaction.execute(
            "DELETE FROM relation_feedback_events WHERE id = ?1",
            [feedback_id_to_i64(id)?],
        )?;
        if matches!(event.kind(), FeedbackKind::Positive) {
            transaction.execute(
                "UPDATE relations SET reinforcement_count = max(reinforcement_count - 1, 0), updated_at_ms = max(updated_at_ms, ?1) WHERE source_id = ?2 AND target_id = ?3 AND name = ?4",
                params![
                    unix_time_ms()?,
                    id_to_i64(event.source())?,
                    id_to_i64(event.target())?,
                    event.relation_name()
                ],
            )?;
        }
        transaction.commit()?;
        Ok(event)
    }

    /// Lists explicit feedback events for one relation in deterministic event order.
    ///
    /// # Errors
    ///
    /// Returns a validation, missing-relation, decoding, or database error.
    pub fn list_relation_feedback(
        &self,
        source: NodeId,
        target: NodeId,
        name: &str,
    ) -> Result<Vec<FeedbackEvent>, RepositoryError> {
        let name = validate_string(name.to_owned(), "relation name", MAX_RELATION_NAME_BYTES)?;
        if self.get_relation(source, target, &name)?.is_none() {
            return Err(RepositoryError::RelationNotFound {
                source,
                target,
                name,
            });
        }
        self.feedback_events_for_relation(source, target, &name)
    }

    pub(crate) fn feedback_events_for_relation(
        &self,
        source: NodeId,
        target: NodeId,
        name: &str,
    ) -> Result<Vec<FeedbackEvent>, RepositoryError> {
        let mut statement = self.connection.prepare(
            "SELECT id, source_id, target_id, relation_name, kind, strength, occurred_at_ms, created_at_ms FROM relation_feedback_events WHERE source_id = ?1 AND target_id = ?2 AND relation_name = ?3 ORDER BY occurred_at_ms, id",
        )?;
        let rows = statement.query_map(
            params![id_to_i64(source)?, id_to_i64(target)?, name],
            read_feedback_event_row,
        )?;
        rows.map(|row| decode_feedback_event(row?)).collect()
    }

    fn open_or_migrate(&mut self) -> Result<(), RepositoryError> {
        let metadata_exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'memory3d_schema')",
            [],
            |row| row.get(0),
        )?;

        if !metadata_exists {
            let user_table_count: i64 = self.connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )?;
            if user_table_count != 0 {
                return Err(RepositoryError::InvalidDatabase(
                    "schema metadata is missing".to_owned(),
                ));
            }
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "CREATE TABLE memory3d_schema (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), format_id TEXT NOT NULL, schema_version INTEGER NOT NULL CHECK (schema_version >= 0), created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0));",
            )?;
            transaction.execute(
                "INSERT INTO memory3d_schema (singleton, format_id, schema_version, created_at_ms) VALUES (1, ?1, 0, ?2)",
                params![FORMAT_ID, unix_time_ms()?],
            )?;
            migrate_to_current(&transaction, 0)?;
            transaction.commit()?;
            return Ok(());
        }

        let (format_id, version): (String, i64) = self.connection.query_row(
            "SELECT format_id, schema_version FROM memory3d_schema WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if format_id != FORMAT_ID {
            return Err(RepositoryError::InvalidDatabase(format!(
                "format identifier is {format_id:?}"
            )));
        }
        if version > SCHEMA_VERSION {
            return Err(RepositoryError::UnsupportedSchema {
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        if !(0..=SCHEMA_VERSION).contains(&version) {
            return Err(RepositoryError::InvalidDatabase(format!(
                "unsupported historical schema version {version}"
            )));
        }
        if version >= 1 {
            validate_stored_coordinates(&self.connection)?;
        }
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        let transaction = self.connection.transaction()?;
        migrate_to_current(&transaction, version)?;
        transaction.commit()?;
        Ok(())
    }
}

fn validate_stored_coordinates(connection: &Connection) -> Result<(), RepositoryError> {
    let mut statement = connection.prepare(
        "SELECT id FROM nodes
         WHERE (x IS NULL) <> (y IS NULL)
            OR (x IS NULL) <> (z IS NULL)
            OR (x IS NOT NULL AND (typeof(x) <> 'real' OR x < -1000000.0 OR x > 1000000.0))
            OR (y IS NOT NULL AND (typeof(y) <> 'real' OR y < -1000000.0 OR y > 1000000.0))
            OR (z IS NOT NULL AND (typeof(z) <> 'real' OR z < -1000000.0 OR z > 1000000.0))
         ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
    if let Some(id) = rows.into_iter().next() {
        return Err(RepositoryError::InvalidDatabase(format!(
            "node {} has invalid coordinates",
            id?
        )));
    }
    Ok(())
}

fn migrate_to_current(transaction: &Transaction<'_>, version: i64) -> Result<(), RepositoryError> {
    if version == 0 {
        migrate_v0_to_v1(transaction)?;
    }
    if version <= 1 {
        migrate_v1_to_v2(transaction)?;
    }
    if version <= 2 {
        migrate_v2_to_v3(transaction)?;
    }
    if version <= 3 {
        migrate_v3_to_v4(transaction)?;
    }
    if version <= 4 {
        migrate_v4_to_v5(transaction)?;
    }
    if version <= 5 {
        migrate_v5_to_v6(transaction)?;
    }
    if version <= 6 {
        migrate_v6_to_v7(transaction)?;
    }
    Ok(())
}

fn migrate_v1_to_v2(transaction: &Transaction<'_>) -> Result<(), RepositoryError> {
    transaction.execute_batch(CREATE_SCHEMA_V2)?;
    let nodes = {
        let mut statement = transaction.prepare("SELECT id, kind, text FROM nodes ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (id, kind, text) in nodes {
        index_node_terms(transaction, id_from_i64(id)?, &kind, &text)?;
    }
    transaction.execute(
        "UPDATE memory3d_schema SET schema_version = ?1 WHERE singleton = 1 AND schema_version = 1",
        [2],
    )?;
    Ok(())
}

fn migrate_v2_to_v3(transaction: &Transaction<'_>) -> Result<(), RepositoryError> {
    transaction.execute_batch(CREATE_SCHEMA_V3)?;
    transaction.execute(
        "UPDATE memory3d_schema SET schema_version = 3 WHERE singleton = 1 AND schema_version = 2",
        [],
    )?;
    Ok(())
}

fn migrate_v3_to_v4(transaction: &Transaction<'_>) -> Result<(), RepositoryError> {
    transaction.execute_batch(CREATE_SCHEMA_V4)?;
    transaction.execute(
        "UPDATE memory3d_schema SET schema_version = 4 WHERE singleton = 1 AND schema_version = 3",
        [],
    )?;
    Ok(())
}

fn migrate_v4_to_v5(transaction: &Transaction<'_>) -> Result<(), RepositoryError> {
    transaction.execute_batch(CREATE_SCHEMA_V5)?;
    transaction.execute(
        "UPDATE memory3d_schema SET schema_version = 5 WHERE singleton = 1 AND schema_version = 4",
        [],
    )?;
    Ok(())
}

fn migrate_v5_to_v6(transaction: &Transaction<'_>) -> Result<(), RepositoryError> {
    transaction.execute_batch(CREATE_SCHEMA_V6)?;
    transaction.execute(
        "UPDATE memory3d_schema SET schema_version = 6 WHERE singleton = 1 AND schema_version = 5",
        [],
    )?;
    Ok(())
}

fn migrate_v6_to_v7(transaction: &Transaction<'_>) -> Result<(), RepositoryError> {
    transaction.execute_batch(
        r"
        CREATE TABLE nodes_v7 (
            id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (id > 0),
            kind TEXT NOT NULL CHECK (length(kind) > 0 AND length(CAST(kind AS BLOB)) <= 64),
            text TEXT NOT NULL CHECK (length(trim(text)) > 0 AND length(CAST(text AS BLOB)) <= 1048576),
            metadata TEXT NOT NULL CHECK (
                json_valid(metadata) AND json_type(metadata) = 'object'
                AND length(CAST(metadata AS BLOB)) <= 65536
            ),
            importance REAL NOT NULL CHECK (importance >= 0.0 AND importance <= 1.0),
            created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
            updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
            x REAL,
            y REAL,
            z REAL,
            CHECK ((x IS NULL AND y IS NULL AND z IS NULL) OR
                   (x IS NOT NULL AND y IS NOT NULL AND z IS NOT NULL)),
            CHECK (x IS NULL OR (typeof(x) = 'real' AND x >= -1000000.0 AND x <= 1000000.0)),
            CHECK (y IS NULL OR (typeof(y) = 'real' AND y >= -1000000.0 AND y <= 1000000.0)),
            CHECK (z IS NULL OR (typeof(z) = 'real' AND z >= -1000000.0 AND z <= 1000000.0))
        );
        INSERT INTO nodes_v7
            SELECT id, kind, text, metadata, importance, created_at_ms, updated_at_ms, x, y, z
            FROM nodes;
        DROP TABLE nodes;
        ALTER TABLE nodes_v7 RENAME TO nodes;
        CREATE TRIGGER community_nodes_insert_revision
        AFTER INSERT ON nodes BEGIN
            UPDATE community_graph_state SET graph_revision = graph_revision + 1 WHERE singleton = 1;
        END;
        CREATE TRIGGER community_nodes_update_revision
        AFTER UPDATE ON nodes BEGIN
            UPDATE community_graph_state SET graph_revision = graph_revision + 1 WHERE singleton = 1;
        END;
        CREATE TRIGGER community_nodes_delete_revision
        AFTER DELETE ON nodes BEGIN
            UPDATE community_graph_state SET graph_revision = graph_revision + 1 WHERE singleton = 1;
        END;
        ",
    )?;
    transaction.execute(
        "UPDATE memory3d_schema SET schema_version = 7 WHERE singleton = 1 AND schema_version = 6",
        [],
    )?;
    Ok(())
}

pub(crate) fn index_node_terms(
    transaction: &Transaction<'_>,
    id: NodeId,
    kind: &str,
    text: &str,
) -> Result<(), RepositoryError> {
    let mut terms = normalize_terms(kind);
    terms.extend(normalize_terms(text));
    terms.sort_unstable();
    terms.dedup();
    for term in terms {
        transaction.execute(
            "INSERT INTO node_terms (node_id, term) VALUES (?1, ?2)",
            params![id_to_i64(id)?, term],
        )?;
    }
    Ok(())
}

fn migrate_v0_to_v1(transaction: &Transaction<'_>) -> Result<(), RepositoryError> {
    transaction.execute_batch(CREATE_SCHEMA_V1)?;
    transaction.execute(
        "UPDATE memory3d_schema SET schema_version = ?1 WHERE singleton = 1 AND schema_version = 0",
        [1],
    )?;
    Ok(())
}

pub(crate) fn unix_time_ms() -> Result<i64, RepositoryError> {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    i64::try_from(millis).map_err(|_| {
        RepositoryError::InvalidDatabase("system time exceeds SQLite integer range".to_owned())
    })
}

pub(crate) fn id_to_i64(id: NodeId) -> Result<i64, RepositoryError> {
    i64::try_from(id.get()).map_err(|_| {
        RepositoryError::InvalidDatabase(format!("node ID {id} exceeds the storage range"))
    })
}

pub(crate) fn id_from_i64(value: i64) -> Result<NodeId, RepositoryError> {
    let value = u128::try_from(value).map_err(|_| {
        RepositoryError::InvalidDatabase(format!("stored node ID {value} is invalid"))
    })?;
    NodeId::new(value).map_err(RepositoryError::from)
}

fn coordinates_parts(coordinates: Option<Coordinates>) -> (Option<f64>, Option<f64>, Option<f64>) {
    coordinates.map_or((None, None, None), |value| {
        (Some(value.x()), Some(value.y()), Some(value.z()))
    })
}

fn ensure_node_exists(transaction: &Transaction<'_>, id: NodeId) -> Result<(), RepositoryError> {
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = ?1)",
        [id_to_i64(id)?],
        |row| row.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(RepositoryError::NodeNotFound(id))
    }
}

fn ensure_relation_exists(
    transaction: &Transaction<'_>,
    source: NodeId,
    target: NodeId,
    name: &str,
) -> Result<(), RepositoryError> {
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM relations WHERE source_id = ?1 AND target_id = ?2 AND name = ?3)",
        params![id_to_i64(source)?, id_to_i64(target)?, name],
        |row| row.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(RepositoryError::RelationNotFound {
            source,
            target,
            name: name.to_owned(),
        })
    }
}

pub(crate) fn feedback_id_to_i64(id: FeedbackEventId) -> Result<i64, RepositoryError> {
    i64::try_from(id.get()).map_err(|_| {
        RepositoryError::InvalidDatabase(format!("feedback event ID {id} exceeds storage range"))
    })
}

pub(crate) fn feedback_id_from_i64(value: i64) -> Result<FeedbackEventId, RepositoryError> {
    let value = u128::try_from(value).map_err(|_| {
        RepositoryError::InvalidDatabase(format!("stored feedback event ID {value} is invalid"))
    })?;
    FeedbackEventId::new(value).map_err(RepositoryError::from)
}

type RawNode = (
    i64,
    String,
    String,
    String,
    f32,
    i64,
    i64,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

fn read_node_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawNode> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

fn decode_node(raw: RawNode) -> Result<MemoryNode, RepositoryError> {
    let (id, kind, text, metadata, importance, created, updated, x, y, z) = raw;
    let id = id_from_i64(id)?;
    let kind = MemoryKind::new(kind)?;
    let metadata = Metadata::new(serde_json::from_str(&metadata).map_err(|error| {
        RepositoryError::InvalidDatabase(format!("node {id} has invalid metadata: {error}"))
    })?)?;
    let mut new = NewMemory::new(kind, text, importance)?.with_metadata(metadata);
    match (x, y, z) {
        (None, None, None) => {}
        (Some(x), Some(y), Some(z)) => {
            new = new.with_coordinates(Coordinates::new(x, y, z)?);
        }
        _ => {
            return Err(RepositoryError::InvalidDatabase(format!(
                "node {id} has partial coordinates"
            )));
        }
    }
    if created < 0 || updated < created {
        return Err(RepositoryError::InvalidDatabase(format!(
            "node {id} has invalid timestamps"
        )));
    }
    Ok(MemoryNode::from_stored(id, new, created, updated))
}

type RawRelation = (i64, i64, String, f32, i64, i64, i64);

pub(crate) fn read_relation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRelation> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

pub(crate) fn decode_relation(raw: RawRelation) -> Result<Relation, RepositoryError> {
    let (source, target, name, weight, created, updated, reinforcement) = raw;
    let new = NewRelation::new(id_from_i64(source)?, id_from_i64(target)?, name, weight)?;
    if created < 0 || updated < created || reinforcement < 0 {
        return Err(RepositoryError::InvalidDatabase(
            "relation has invalid counters or timestamps".to_owned(),
        ));
    }
    let reinforcement = u64::try_from(reinforcement).map_err(|_| {
        RepositoryError::InvalidDatabase("relation has a negative reinforcement count".to_owned())
    })?;
    Ok(Relation::from_stored(new, created, updated, reinforcement))
}

type RawFeedbackEvent = (i64, i64, i64, String, String, f32, i64, i64);

pub(crate) fn read_feedback_event_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RawFeedbackEvent> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

pub(crate) fn decode_feedback_event(
    raw: RawFeedbackEvent,
) -> Result<FeedbackEvent, RepositoryError> {
    let (id, source, target, relation_name, kind, strength, occurred, created) = raw;
    let new = NewFeedbackEvent::new(
        id_from_i64(source)?,
        id_from_i64(target)?,
        relation_name,
        FeedbackKind::from_str(&kind)?,
        strength,
        occurred,
    )?;
    if created < 0 {
        return Err(RepositoryError::InvalidDatabase(
            "feedback event has invalid created timestamp".to_owned(),
        ));
    }
    Ok(FeedbackEvent::from_stored(
        feedback_id_from_i64(id)?,
        new,
        created,
    ))
}

fn get_feedback_event_in_transaction(
    transaction: &Transaction<'_>,
    id: FeedbackEventId,
) -> Result<Option<FeedbackEvent>, RepositoryError> {
    let raw = transaction
        .query_row(
            "SELECT id, source_id, target_id, relation_name, kind, strength, occurred_at_ms, created_at_ms FROM relation_feedback_events WHERE id = ?1",
            [feedback_id_to_i64(id)?],
            read_feedback_event_row,
        )
        .optional()?;
    raw.map(decode_feedback_event).transpose()
}
