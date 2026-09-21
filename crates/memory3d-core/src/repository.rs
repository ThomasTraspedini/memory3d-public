use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, ffi::ErrorCode, params};

use crate::{
    Coordinates, MAX_RELATION_NAME_BYTES, MemoryKind, MemoryNode, Metadata, NewMemory, NewRelation,
    NodeId, Relation, ValidationError, normalize_terms, validate_string, validate_unit_interval,
};

const FORMAT_ID: &str = "memory3d";
const SCHEMA_VERSION: i64 = 3;
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
    CHECK (x IS NULL OR (x >= -1.7976931348623157e308 AND x <= 1.7976931348623157e308)),
    CHECK (y IS NULL OR (y >= -1.7976931348623157e308 AND y <= 1.7976931348623157e308)),
    CHECK (z IS NULL OR (z >= -1.7976931348623157e308 AND z <= 1.7976931348623157e308))
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
            Self::Clock(error) => write!(formatter, "system clock error: {error}"),
        }
    }
}

impl Error for RepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Database(error) => Some(error),
            Self::Clock(error) => Some(error),
            Self::Busy
            | Self::InvalidDatabase(_)
            | Self::UnsupportedSchema { .. }
            | Self::NodeNotFound(_)
            | Self::RelationNotFound { .. } => None,
        }
    }
}

impl From<ValidationError> for RepositoryError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
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
        connection.pragma_update(None, "foreign_keys", true)?;
        let mut repository = Self {
            connection,
            path: path.to_path_buf(),
        };
        repository.open_or_migrate()?;
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
            migrate_v0_to_v1(&transaction)?;
            migrate_v1_to_v2(&transaction)?;
            migrate_v2_to_v3(&transaction)?;
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
        match version {
            0 => {
                let transaction = self.connection.transaction()?;
                migrate_v0_to_v1(&transaction)?;
                migrate_v1_to_v2(&transaction)?;
                migrate_v2_to_v3(&transaction)?;
                transaction.commit()?;
                Ok(())
            }
            1 => {
                let transaction = self.connection.transaction()?;
                migrate_v1_to_v2(&transaction)?;
                migrate_v2_to_v3(&transaction)?;
                transaction.commit()?;
                Ok(())
            }
            2 => {
                let transaction = self.connection.transaction()?;
                migrate_v2_to_v3(&transaction)?;
                transaction.commit()?;
                Ok(())
            }
            SCHEMA_VERSION => Ok(()),
            _ => Err(RepositoryError::InvalidDatabase(format!(
                "unsupported historical schema version {version}"
            ))),
        }
    }
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

fn index_node_terms(
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

fn unix_time_ms() -> Result<i64, RepositoryError> {
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
