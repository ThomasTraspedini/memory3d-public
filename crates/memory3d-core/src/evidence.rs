//! Bounded, reviewable evidence-envelope types and durable workflow.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::{
    ActivationOptions, ActivationPath, DatabaseStats, MAX_KIND_BYTES, MAX_RESULTS, MAX_TEXT_BYTES,
    MemoryKind, MemoryNode, NodeId, Repository, RepositoryError, TraversalStats, ValidationError,
    repository::{id_from_i64, id_to_i64, index_node_terms, unix_time_ms},
    validate_range,
};

/// Version of the textual evidence-bundle contract implemented by this core.
pub const EVIDENCE_BUNDLE_VERSION: u32 = 1;
/// Maximum encoded UTF-8 size of one textual bundle.
pub const MAX_BUNDLE_BYTES: usize = 4_194_304;
/// Maximum number of evidence items admitted by one bundle.
pub const MAX_BUNDLE_ITEMS: usize = 256;
/// Maximum byte length of an evidence or idempotency identifier.
pub const MAX_EVIDENCE_ID_BYTES: usize = 128;
/// Maximum byte length of a source, producer, actor, or policy identity.
pub const MAX_IDENTITY_BYTES: usize = 256;
/// Maximum number of exact key/value selectors in one scope.
pub const MAX_SCOPE_ENTRIES: usize = 16;
/// Maximum references of one kind from one evidence item.
pub const MAX_EVIDENCE_REFERENCES: usize = 64;
/// Maximum encoded text bytes admitted to one context package.
pub const MAX_CONTEXT_BYTES: usize = 1_048_576;
/// Maximum ranked activation candidates inspected by one evidence-context assembly.
pub const MAX_EVIDENCE_CANDIDATE_SCAN_LIMIT: usize = 1_000;
/// Conservative lexical/graph relevance floor selected by the Task 27 parameter replay.
pub const DEFAULT_EVIDENCE_MIN_RELEVANCE: f32 = 0.5;
const MAX_SCOPE_KEY_BYTES: usize = 128;
const MAX_SCOPE_VALUE_BYTES: usize = 256;

/// Structural failures detected before an evidence bundle reaches persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceError {
    /// The textual DTO is not valid JSON or does not match the versioned shape.
    InvalidJson(String),
    /// The bundle uses a version this core does not implement.
    UnsupportedVersion(u32),
    /// A required field is blank or exceeds its documented byte bound.
    InvalidField {
        /// Name of the invalid DTO field.
        field: &'static str,
        /// Human-readable reason safe to expose through adapters.
        reason: String,
    },
    /// A bounded collection is empty or exceeds its hard limit.
    InvalidCount {
        /// Name of the invalid collection.
        field: &'static str,
        /// Observed element count.
        found: usize,
        /// Inclusive minimum.
        min: usize,
        /// Inclusive maximum.
        max: usize,
    },
    /// Two items in one bundle declare the same stable evidence ID.
    DuplicateEvidenceId(String),
    /// One reference points back to the item that declares it.
    SelfReference {
        /// Referencing evidence ID.
        evidence_id: String,
        /// Reference category.
        relation: &'static str,
    },
    /// A decision-bearing lifecycle or transition lacks decision provenance.
    DecisionProvenanceRequired(String),
    /// Conflict references were supplied without a stable conflict group.
    ConflictGroupRequired(String),
}

impl std::fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(reason) => {
                write!(formatter, "invalid evidence bundle JSON: {reason}")
            }
            Self::UnsupportedVersion(version) => write!(
                formatter,
                "evidence bundle version {version} is unsupported; expected {EVIDENCE_BUNDLE_VERSION}"
            ),
            Self::InvalidField { field, reason } => {
                write!(formatter, "invalid evidence field {field}: {reason}")
            }
            Self::InvalidCount {
                field,
                found,
                min,
                max,
            } => write!(
                formatter,
                "evidence field {field} has {found} items; expected {min}..={max}"
            ),
            Self::DuplicateEvidenceId(id) => write!(formatter, "duplicate evidence ID {id:?}"),
            Self::SelfReference {
                evidence_id,
                relation,
            } => write!(
                formatter,
                "evidence {evidence_id:?} may not reference itself through {relation}"
            ),
            Self::DecisionProvenanceRequired(id) => write!(
                formatter,
                "evidence {id:?} requires explicit decision provenance"
            ),
            Self::ConflictGroupRequired(id) => write!(
                formatter,
                "evidence {id:?} has conflict references but no conflict group"
            ),
        }
    }
}

impl std::error::Error for EvidenceError {}

/// Explicit lifecycle attached to every evidence item independently from relevance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLifecycle {
    /// Proposed material awaiting review.
    Candidate,
    /// Directly or deterministically recorded observation.
    Observed,
    /// Evidence accepted as independently supported by a declared policy.
    Corroborated,
    /// Evidence accepted for a declared scope by an authorized decision.
    Approved,
    /// Evidence explicitly contradicted without deleting either side.
    Contradicted,
    /// Evidence explicitly replaced by a later decision.
    Superseded,
    /// Evidence explicitly rejected while retained for audit.
    Rejected,
}

impl EvidenceLifecycle {
    /// Stable textual label used by the DTO, storage, and adapters.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Observed => "observed",
            Self::Corroborated => "corroborated",
            Self::Approved => "approved",
            Self::Contradicted => "contradicted",
            Self::Superseded => "superseded",
            Self::Rejected => "rejected",
        }
    }

    fn from_stored(value: &str) -> Result<Self, RepositoryError> {
        match value {
            "candidate" => Ok(Self::Candidate),
            "observed" => Ok(Self::Observed),
            "corroborated" => Ok(Self::Corroborated),
            "approved" => Ok(Self::Approved),
            "contradicted" => Ok(Self::Contradicted),
            "superseded" => Ok(Self::Superseded),
            "rejected" => Ok(Self::Rejected),
            _ => Err(RepositoryError::InvalidDatabase(format!(
                "stored evidence lifecycle {value:?} is invalid"
            ))),
        }
    }

    const fn requires_decision(self) -> bool {
        matches!(
            self,
            Self::Corroborated
                | Self::Approved
                | Self::Contradicted
                | Self::Superseded
                | Self::Rejected
        )
    }
}

/// Actor/policy record for a lifecycle decision or supersession.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionProvenance {
    actor: String,
    policy: String,
    supporting_evidence_ids: Vec<String>,
    decided_at_ms: i64,
}

impl DecisionProvenance {
    /// Returns the responsible actor identity.
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }
    /// Returns the explicit policy identity.
    #[must_use]
    pub fn policy(&self) -> &str {
        &self.policy
    }
    /// Returns stable evidence IDs supporting the decision.
    #[must_use]
    pub fn supporting_evidence_ids(&self) -> &[String] {
        &self.supporting_evidence_ids
    }
    /// Returns the caller-supplied decision time in Unix epoch milliseconds.
    #[must_use]
    pub const fn decided_at_ms(&self) -> i64 {
        self.decided_at_ms
    }
}

/// One validated immutable assertion or observation in an evidence bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceItem {
    id: String,
    kind: MemoryKind,
    text: String,
    source: String,
    producer: String,
    scope: BTreeMap<String, String>,
    lifecycle: EvidenceLifecycle,
    created_at_ms: i64,
    observed_at_ms: Option<i64>,
    derives_from: Vec<String>,
    conflict_group: Option<String>,
    conflicts_with: Vec<String>,
    supersedes: Vec<String>,
    decision: Option<DecisionProvenance>,
}

impl EvidenceItem {
    /// Returns the immutable external evidence ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Returns the caller-defined domain-neutral kind.
    #[must_use]
    pub const fn kind(&self) -> &MemoryKind {
        &self.kind
    }
    /// Returns the raw textual assertion or observation.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Returns the supporting source identity.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
    /// Returns the actor or tool that produced the envelope item.
    #[must_use]
    pub fn producer(&self) -> &str {
        &self.producer
    }
    /// Returns the exact applicable-scope selectors.
    #[must_use]
    pub const fn scope(&self) -> &BTreeMap<String, String> {
        &self.scope
    }
    /// Returns the explicit raw lifecycle state.
    #[must_use]
    pub const fn lifecycle(&self) -> EvidenceLifecycle {
        self.lifecycle
    }
    /// Returns the caller-supplied creation time.
    #[must_use]
    pub const fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }
    /// Returns the optional observation/effective time.
    #[must_use]
    pub const fn observed_at_ms(&self) -> Option<i64> {
        self.observed_at_ms
    }
    /// Returns evidence IDs from which this item was derived.
    #[must_use]
    pub fn derives_from(&self) -> &[String] {
        &self.derives_from
    }
    /// Returns the stable conflict-group identity, when declared.
    #[must_use]
    pub fn conflict_group(&self) -> Option<&str> {
        self.conflict_group.as_deref()
    }
    /// Returns explicitly competing evidence IDs.
    #[must_use]
    pub fn conflicts_with(&self) -> &[String] {
        &self.conflicts_with
    }
    /// Returns evidence IDs this newer item explicitly supersedes.
    #[must_use]
    pub fn supersedes(&self) -> &[String] {
        &self.supersedes
    }
    /// Returns decision provenance when the lifecycle or transition requires it.
    #[must_use]
    pub const fn decision(&self) -> Option<&DecisionProvenance> {
        self.decision.as_ref()
    }
}

/// Versioned, canonical textual input reviewed before application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceBundle {
    version: u32,
    idempotency_key: String,
    items: Vec<EvidenceItem>,
    canonical_json: String,
}

impl EvidenceBundle {
    /// Parses and validates the versioned JSON DTO, then produces a deterministic canonical form.
    ///
    /// # Errors
    ///
    /// Returns a typed structural error without opening or mutating a repository.
    pub fn from_json(json: &str) -> Result<Self, EvidenceError> {
        if json.len() > MAX_BUNDLE_BYTES {
            return Err(EvidenceError::InvalidField {
                field: "bundle",
                reason: format!("exceeds {MAX_BUNDLE_BYTES} UTF-8 bytes"),
            });
        }
        let mut raw: RawBundle = serde_json::from_str(json)
            .map_err(|error| EvidenceError::InvalidJson(error.to_string()))?;
        if raw.version != EVIDENCE_BUNDLE_VERSION {
            return Err(EvidenceError::UnsupportedVersion(raw.version));
        }
        validate_count("items", raw.items.len(), 1, MAX_BUNDLE_ITEMS)?;
        raw.idempotency_key = validate_text(
            raw.idempotency_key,
            "idempotency_key",
            MAX_EVIDENCE_ID_BYTES,
        )?;
        raw.items.sort_by(|left, right| left.id.cmp(&right.id));
        let mut ids = BTreeSet::new();
        let mut items = Vec::with_capacity(raw.items.len());
        for item in &mut raw.items {
            normalize_references(item);
            if !ids.insert(item.id.clone()) {
                return Err(EvidenceError::DuplicateEvidenceId(item.id.clone()));
            }
            items.push(validate_item(item)?);
        }
        let canonical_json = serde_json::to_string(&raw)
            .map_err(|error| EvidenceError::InvalidJson(error.to_string()))?;
        if canonical_json.len() > MAX_BUNDLE_BYTES {
            return Err(EvidenceError::InvalidField {
                field: "bundle",
                reason: format!("canonical form exceeds {MAX_BUNDLE_BYTES} UTF-8 bytes"),
            });
        }
        Ok(Self {
            version: raw.version,
            idempotency_key: raw.idempotency_key,
            items,
            canonical_json,
        })
    }

    /// Returns the implemented DTO version.
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }
    /// Returns the stable idempotency key.
    #[must_use]
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }
    /// Returns validated items ordered by stable evidence ID.
    #[must_use]
    pub fn items(&self) -> &[EvidenceItem] {
        &self.items
    }
    /// Returns the normalized JSON used for exact idempotency comparison.
    #[must_use]
    pub fn canonical_json(&self) -> &str {
        &self.canonical_json
    }
    /// Returns a deterministic non-cryptographic fingerprint for operator previews.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        format!("fnv1a64:{:016x}", fnv1a64(self.canonical_json.as_bytes()))
    }
}

/// Explicit authorization required by transactional bundle application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyAuthorization {
    actor: String,
    policy: String,
}

impl ApplyAuthorization {
    /// Creates bounded actor and policy identities.
    ///
    /// # Errors
    ///
    /// Returns an evidence validation error for blank or oversized identities.
    pub fn new(actor: impl Into<String>, policy: impl Into<String>) -> Result<Self, EvidenceError> {
        Ok(Self {
            actor: validate_text(actor.into(), "apply.actor", MAX_IDENTITY_BYTES)?,
            policy: validate_text(policy.into(), "apply.policy", MAX_IDENTITY_BYTES)?,
        })
    }
    /// Returns the applying actor.
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }
    /// Returns the explicit apply policy.
    #[must_use]
    pub fn policy(&self) -> &str {
        &self.policy
    }
}

/// Whether preview describes a new transaction or an exact prior application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundlePreviewStatus {
    /// The key and evidence IDs are unused and can be applied.
    New,
    /// The same key already identifies the exact canonical active payload.
    IdempotentReplay,
}

impl BundlePreviewStatus {
    /// Stable adapter label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::IdempotentReplay => "idempotent_replay",
        }
    }
}

/// Deterministic, read-only diff produced before bundle application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundlePreview {
    /// New or replay state.
    pub status: BundlePreviewStatus,
    /// Idempotency key under review.
    pub idempotency_key: String,
    /// Canonical payload fingerprint.
    pub fingerprint: String,
    /// Evidence IDs that would be added, in stable order.
    pub evidence_ids: Vec<String>,
    /// Nodes projected into ordinary bounded retrieval.
    pub projected_nodes: usize,
    /// Typed evidence references projected as graph relations.
    pub projected_relations: usize,
}

/// Stable evidence-to-node mapping returned by apply and reopen-safe lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedEvidenceItem {
    /// External immutable evidence ID.
    pub evidence_id: String,
    /// Stable ordinary graph node ID used for bounded relevance retrieval.
    pub node_id: NodeId,
}

/// Result of an atomic application or exact idempotent replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleApplyReport {
    /// Durable numeric bundle record.
    pub bundle_id: u64,
    /// Stable idempotency key.
    pub idempotency_key: String,
    /// Whether this call replayed an identical active application.
    pub replayed: bool,
    /// Evidence-to-node mappings ordered by evidence ID.
    pub items: Vec<AppliedEvidenceItem>,
    /// Durable application time.
    pub applied_at_ms: i64,
}

/// Result of an explicit compensating rollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleRollbackReport {
    /// Stable idempotency key retained in the rollback ledger.
    pub idempotency_key: String,
    /// Number of immutable items removed by the explicit compensation.
    pub removed_items: usize,
    /// Number of graph nodes removed with those items.
    pub removed_nodes: usize,
    /// Durable rollback time.
    pub rolled_back_at_ms: i64,
}

/// Reopen-safe evidence record plus its bounded-retrieval projection.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEvidence {
    /// Validated immutable evidence data.
    pub evidence: EvidenceItem,
    /// Stable projected memory node.
    pub node: MemoryNode,
    /// Bundle idempotency key that introduced this record.
    pub bundle_key: String,
    /// Durable application timestamp.
    pub applied_at_ms: i64,
}

/// Explicit reason a bounded candidate was kept outside admitted context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextExclusionReason {
    /// Item scope was absent or differed from the exact query scope.
    ScopeMismatch,
    /// Caller policy did not admit the item's lifecycle.
    Lifecycle,
    /// Candidate relevance is below the declared relevance-only admission floor.
    LowRelevance,
    /// Item belongs to a visible unresolved conflict.
    Conflict,
    /// A later stored item explicitly supersedes this item.
    Superseded,
    /// Adding the item would exceed the package byte cap.
    SizeLimit,
}

impl ContextExclusionReason {
    /// Stable adapter label.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ScopeMismatch => "scope_mismatch",
            Self::Lifecycle => "lifecycle",
            Self::LowRelevance => "low_relevance",
            Self::Conflict => "conflict",
            Self::Superseded => "superseded",
            Self::SizeLimit => "size_limit",
        }
    }
}

/// Policy and hard bounds for one evidence context assembly.
#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceContextOptions {
    scope: BTreeMap<String, String>,
    accepted_lifecycles: BTreeSet<EvidenceLifecycle>,
    activation: ActivationOptions,
    result_limit: usize,
    candidate_scan_limit: usize,
    max_bytes: usize,
    min_relevance_score: f32,
}

impl EvidenceContextOptions {
    /// Creates the conservative settled-context policy.
    ///
    /// It accepts only exact-scope `observed`, `corroborated`, and `approved` evidence while
    /// excluding every unresolved conflict and every item superseded by later stored evidence.
    ///
    /// # Errors
    ///
    /// Returns an evidence validation error for an empty, oversized, or malformed scope.
    pub fn settled(scope: BTreeMap<String, String>) -> Result<Self, EvidenceError> {
        validate_scope(&scope, "context.scope")?;
        Ok(Self {
            scope,
            accepted_lifecycles: [
                EvidenceLifecycle::Observed,
                EvidenceLifecycle::Corroborated,
                EvidenceLifecycle::Approved,
            ]
            .into_iter()
            .collect(),
            activation: ActivationOptions {
                include_seeds: true,
                ..ActivationOptions::default()
            },
            result_limit: 10,
            candidate_scan_limit: 100,
            max_bytes: MAX_CONTEXT_BYTES,
            min_relevance_score: DEFAULT_EVIDENCE_MIN_RELEVANCE,
        })
    }

    /// Replaces the admitted lifecycle set while preserving explicit status in every result.
    ///
    /// # Errors
    ///
    /// Returns an evidence validation error when the set is empty.
    pub fn with_accepted_lifecycles(
        mut self,
        states: impl IntoIterator<Item = EvidenceLifecycle>,
    ) -> Result<Self, EvidenceError> {
        self.accepted_lifecycles = states.into_iter().collect();
        validate_count(
            "context.accepted_lifecycles",
            self.accepted_lifecycles.len(),
            1,
            7,
        )?;
        Ok(self)
    }

    /// Replaces bounded graph-relevance options; seeds remain visible for scope/status filtering.
    ///
    /// The activation `limit` is retained as this context's admitted result limit for adapter
    /// compatibility, but evidence scans the internal ranked stream before that ordinary
    /// activation truncation.
    #[must_use]
    pub fn with_activation(mut self, mut activation: ActivationOptions) -> Self {
        activation.include_seeds = true;
        self.result_limit = activation.limit;
        self.activation = activation;
        self
    }

    /// Replaces the maximum number of evidence items admitted to this package.
    #[must_use]
    pub fn with_result_limit(mut self, result_limit: usize) -> Self {
        self.result_limit = result_limit;
        self
    }

    /// Replaces the maximum ranked activation candidates inspected by evidence policy.
    #[must_use]
    pub fn with_candidate_scan_limit(mut self, candidate_scan_limit: usize) -> Self {
        self.candidate_scan_limit = candidate_scan_limit;
        self
    }

    fn validate(&self) -> Result<(), ValidationError> {
        self.activation.validate()?;
        validate_range(self.result_limit, "evidence_result_limit", 1, MAX_RESULTS)?;
        validate_range(
            self.candidate_scan_limit,
            "evidence_candidate_scan_limit",
            1,
            MAX_EVIDENCE_CANDIDATE_SCAN_LIMIT,
        )
    }

    /// Replaces the encoded context cap.
    ///
    /// # Errors
    ///
    /// Returns an evidence validation error outside `1..=MAX_CONTEXT_BYTES`.
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Result<Self, EvidenceError> {
        if !(1..=MAX_CONTEXT_BYTES).contains(&max_bytes) {
            return Err(EvidenceError::InvalidCount {
                field: "context.max_bytes",
                found: max_bytes,
                min: 1,
                max: MAX_CONTEXT_BYTES,
            });
        }
        self.max_bytes = max_bytes;
        Ok(self)
    }

    /// Replaces the relevance-only admission floor without changing epistemic status policy.
    ///
    /// # Errors
    ///
    /// Returns an evidence validation error unless the score is finite and in `[0, 1]`.
    pub fn with_min_relevance_score(mut self, score: f32) -> Result<Self, EvidenceError> {
        if !score.is_finite() || !(0.0..=1.0).contains(&score) {
            return Err(EvidenceError::InvalidField {
                field: "context.min_relevance_score",
                reason: "must be finite and between 0 and 1 inclusive".to_owned(),
            });
        }
        self.min_relevance_score = score;
        Ok(self)
    }
}

/// Why bounded evidence-context scanning stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextStopReason {
    /// Every candidate produced by the bounded activation traversal was inspected.
    CandidateStreamExhausted,
    /// The configured candidate scan bound was consumed before the stream ended.
    CandidateScanLimitReached,
    /// The configured number of evidence items was admitted.
    ResultLimitReached,
    /// Admitted text exactly filled the byte budget.
    ByteLimitReached,
}

impl ContextStopReason {
    /// Stable adapter label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CandidateStreamExhausted => "candidate_stream_exhausted",
            Self::CandidateScanLimitReached => "candidate_scan_limit_reached",
            Self::ResultLimitReached => "result_limit_reached",
            Self::ByteLimitReached => "byte_limit_reached",
        }
    }
}

/// One ranked context candidate with status, provenance, conflicts, and reconstructable path.
#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceContextItem {
    /// Reopen-safe stored evidence and graph node.
    pub stored: StoredEvidence,
    /// Graph/lexical relevance score; never a truth or authority probability.
    pub relevance_score: f32,
    /// Reconstructable path from the selected lexical seed.
    pub path: ActivationPath,
    /// All visible inbound or outbound conflict references.
    pub conflicts: Vec<String>,
    /// Later evidence IDs that explicitly supersede this item.
    pub superseded_by: Vec<String>,
    /// Stable description of how the candidate was selected.
    pub selection_provenance: &'static str,
    /// Exclusion reason, absent only for admitted items.
    pub exclusion: Option<ContextExclusionReason>,
}

/// Bounded package for a consumer, with admitted and excluded candidates reported separately.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextPackage {
    /// Query supplied to bounded graph relevance selection.
    pub query: String,
    /// Exact scope required by this call.
    pub scope: BTreeMap<String, String>,
    /// Candidates admitted by scope, lifecycle, conflict, supersession, and byte policy.
    pub admitted: Vec<EvidenceContextItem>,
    /// Relevant candidates kept visible but not admitted.
    pub excluded: Vec<EvidenceContextItem>,
    /// True when no evidence item was admitted from the bounded candidate scan.
    pub abstained: bool,
    /// Sum of admitted text byte lengths.
    pub estimated_text_bytes: usize,
    /// Relevance-only admission floor used by this package; never a truth score.
    pub min_relevance_score: f32,
    /// Number of ranked activation candidates inspected.
    pub candidate_work: usize,
    /// Number of inspected ordinary graph nodes, which remain absent from `excluded`.
    pub ordinary_candidates_skipped: usize,
    /// Number of inspected projected evidence candidates evaluated by evidence policy.
    pub evidence_candidates_evaluated: usize,
    /// Configured maximum ranked activation candidates inspected.
    pub candidate_scan_limit: usize,
    /// Configured maximum admitted evidence items.
    pub result_limit: usize,
    /// Explicit reason evidence policy stopped scanning the ranked candidate stream.
    pub stop_reason: ContextStopReason,
    /// Underlying bounded traversal work.
    pub traversal: TraversalStats,
    /// Underlying storage calls and node reads.
    pub database: DatabaseStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBundle {
    version: u32,
    idempotency_key: String,
    items: Vec<RawEvidenceItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEvidenceItem {
    id: String,
    kind: String,
    text: String,
    source: String,
    producer: String,
    scope: BTreeMap<String, String>,
    lifecycle: EvidenceLifecycle,
    created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observed_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    derives_from: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conflict_group: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    conflicts_with: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    supersedes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decision: Option<RawDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDecision {
    actor: String,
    policy: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    supporting_evidence_ids: Vec<String>,
    decided_at_ms: i64,
}

fn normalize_references(item: &mut RawEvidenceItem) {
    for references in [
        &mut item.derives_from,
        &mut item.conflicts_with,
        &mut item.supersedes,
    ] {
        references.sort();
        references.dedup();
    }
    if let Some(decision) = &mut item.decision {
        decision.supporting_evidence_ids.sort();
        decision.supporting_evidence_ids.dedup();
    }
}

fn validate_item(raw: &mut RawEvidenceItem) -> Result<EvidenceItem, EvidenceError> {
    raw.id = validate_text(raw.id.clone(), "items[].id", MAX_EVIDENCE_ID_BYTES)?;
    raw.kind = validate_text(raw.kind.clone(), "items[].kind", MAX_KIND_BYTES)?;
    raw.text = validate_text(raw.text.clone(), "items[].text", MAX_TEXT_BYTES)?;
    raw.source = validate_text(raw.source.clone(), "items[].source", MAX_IDENTITY_BYTES)?;
    raw.producer = validate_text(raw.producer.clone(), "items[].producer", MAX_IDENTITY_BYTES)?;
    validate_scope(&raw.scope, "items[].scope")?;
    validate_timestamp(raw.created_at_ms, "items[].created_at_ms")?;
    if let Some(observed) = raw.observed_at_ms {
        validate_timestamp(observed, "items[].observed_at_ms")?;
    }
    for (relation, references) in [
        ("derives_from", &raw.derives_from),
        ("conflicts_with", &raw.conflicts_with),
        ("supersedes", &raw.supersedes),
    ] {
        validate_references(&raw.id, relation, references)?;
    }
    if !raw.conflicts_with.is_empty() && raw.conflict_group.is_none() {
        return Err(EvidenceError::ConflictGroupRequired(raw.id.clone()));
    }
    if let Some(group) = &mut raw.conflict_group {
        *group = validate_text(
            group.clone(),
            "items[].conflict_group",
            MAX_EVIDENCE_ID_BYTES,
        )?;
    }
    if (raw.lifecycle.requires_decision() || !raw.supersedes.is_empty()) && raw.decision.is_none() {
        return Err(EvidenceError::DecisionProvenanceRequired(raw.id.clone()));
    }
    if let Some(decision) = &raw.decision {
        validate_references(
            &raw.id,
            "decision_support",
            &decision.supporting_evidence_ids,
        )?;
    }
    let decision = raw.decision.as_mut().map(validate_decision).transpose()?;
    Ok(EvidenceItem {
        id: raw.id.clone(),
        kind: MemoryKind::new(raw.kind.clone()).map_err(|error| EvidenceError::InvalidField {
            field: "items[].kind",
            reason: error.to_string(),
        })?,
        text: raw.text.clone(),
        source: raw.source.clone(),
        producer: raw.producer.clone(),
        scope: raw.scope.clone(),
        lifecycle: raw.lifecycle,
        created_at_ms: raw.created_at_ms,
        observed_at_ms: raw.observed_at_ms,
        derives_from: raw.derives_from.clone(),
        conflict_group: raw.conflict_group.clone(),
        conflicts_with: raw.conflicts_with.clone(),
        supersedes: raw.supersedes.clone(),
        decision,
    })
}

fn validate_decision(raw: &mut RawDecision) -> Result<DecisionProvenance, EvidenceError> {
    raw.actor = validate_text(raw.actor.clone(), "decision.actor", MAX_IDENTITY_BYTES)?;
    raw.policy = validate_text(raw.policy.clone(), "decision.policy", MAX_IDENTITY_BYTES)?;
    validate_timestamp(raw.decided_at_ms, "decision.decided_at_ms")?;
    validate_count(
        "decision.supporting_evidence_ids",
        raw.supporting_evidence_ids.len(),
        0,
        MAX_EVIDENCE_REFERENCES,
    )?;
    for id in &mut raw.supporting_evidence_ids {
        *id = validate_text(
            id.clone(),
            "decision.supporting_evidence_ids[]",
            MAX_EVIDENCE_ID_BYTES,
        )?;
    }
    Ok(DecisionProvenance {
        actor: raw.actor.clone(),
        policy: raw.policy.clone(),
        supporting_evidence_ids: raw.supporting_evidence_ids.clone(),
        decided_at_ms: raw.decided_at_ms,
    })
}

fn validate_references(
    item_id: &str,
    relation: &'static str,
    references: &[String],
) -> Result<(), EvidenceError> {
    validate_count(relation, references.len(), 0, MAX_EVIDENCE_REFERENCES)?;
    for reference in references {
        validate_text(
            reference.clone(),
            "items[].references[]",
            MAX_EVIDENCE_ID_BYTES,
        )?;
        if reference == item_id {
            return Err(EvidenceError::SelfReference {
                evidence_id: item_id.to_owned(),
                relation,
            });
        }
    }
    Ok(())
}

fn validate_scope(
    scope: &BTreeMap<String, String>,
    field: &'static str,
) -> Result<(), EvidenceError> {
    validate_count(field, scope.len(), 1, MAX_SCOPE_ENTRIES)?;
    for (key, value) in scope {
        validate_text(key.clone(), "scope.key", MAX_SCOPE_KEY_BYTES)?;
        validate_text(value.clone(), "scope.value", MAX_SCOPE_VALUE_BYTES)?;
    }
    Ok(())
}

fn validate_timestamp(value: i64, field: &'static str) -> Result<(), EvidenceError> {
    if value < 0 {
        Err(EvidenceError::InvalidField {
            field,
            reason: "must be a non-negative Unix epoch millisecond value".to_owned(),
        })
    } else {
        Ok(())
    }
}

fn validate_text(
    value: String,
    field: &'static str,
    max_bytes: usize,
) -> Result<String, EvidenceError> {
    if value.trim().is_empty() {
        return Err(EvidenceError::InvalidField {
            field,
            reason: "must not be blank".to_owned(),
        });
    }
    if value.len() > max_bytes {
        return Err(EvidenceError::InvalidField {
            field,
            reason: format!("must be at most {max_bytes} UTF-8 bytes"),
        });
    }
    Ok(value)
}

fn validate_count(
    field: &'static str,
    found: usize,
    min: usize,
    max: usize,
) -> Result<(), EvidenceError> {
    if (min..=max).contains(&found) {
        Ok(())
    } else {
        Err(EvidenceError::InvalidCount {
            field,
            found,
            min,
            max,
        })
    }
}

const fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

impl Repository {
    /// Validates references and returns a deterministic read-only application diff.
    ///
    /// # Errors
    ///
    /// Returns typed idempotency, immutable-ID, reference, conflict-group, or database failures.
    pub fn preview_evidence_bundle(
        &self,
        bundle: &EvidenceBundle,
    ) -> Result<BundlePreview, RepositoryError> {
        if let Some(existing) = self.bundle_ledger(bundle.idempotency_key())? {
            if existing.canonical_payload != bundle.canonical_json() {
                return Err(RepositoryError::IdempotencyConflict(
                    bundle.idempotency_key().to_owned(),
                ));
            }
            if existing.rolled_back_at_ms.is_some() {
                return Err(RepositoryError::EvidenceBundleRolledBack(
                    bundle.idempotency_key().to_owned(),
                ));
            }
            return Ok(bundle_preview(
                bundle,
                BundlePreviewStatus::IdempotentReplay,
            ));
        }

        let local = bundle
            .items()
            .iter()
            .map(|item| {
                (
                    item.id().to_owned(),
                    item.conflict_group().map(str::to_owned),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for item in bundle.items() {
            if self.active_evidence_exists(item.id())? {
                return Err(RepositoryError::EvidenceIdExists(item.id().to_owned()));
            }
            for (relation, references) in evidence_references(item) {
                for target in references {
                    let target_group = if let Some(group) = local.get(target) {
                        group.clone()
                    } else {
                        let (exists, group) = self.active_conflict_group(target)?;
                        if !exists {
                            return Err(RepositoryError::EvidenceReferenceNotFound {
                                evidence_id: item.id().to_owned(),
                                relation,
                                target: target.clone(),
                            });
                        }
                        group
                    };
                    if relation == "conflicts_with"
                        && target_group.as_deref() != item.conflict_group()
                    {
                        return Err(RepositoryError::EvidenceConflictGroupMismatch {
                            evidence_id: item.id().to_owned(),
                            target: target.clone(),
                        });
                    }
                }
            }
        }
        Ok(bundle_preview(bundle, BundlePreviewStatus::New))
    }

    /// Applies a previewed bundle atomically under explicit operator or policy authorization.
    ///
    /// Identical active replays return the original durable mapping. Reusing a key for another
    /// payload, reusing an evidence ID, or failing any reference rolls back the whole transaction.
    ///
    /// # Errors
    ///
    /// Returns a typed evidence, idempotency, reference, rollback-ledger, or database failure.
    #[allow(clippy::too_many_lines)]
    pub fn apply_evidence_bundle(
        &mut self,
        bundle: &EvidenceBundle,
        authorization: &ApplyAuthorization,
    ) -> Result<BundleApplyReport, RepositoryError> {
        let preview = self.preview_evidence_bundle(bundle)?;
        if preview.status == BundlePreviewStatus::IdempotentReplay {
            return self.load_apply_report(bundle.idempotency_key(), true);
        }

        let now = unix_time_ms()?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO evidence_bundles (idempotency_key, canonical_payload, applied_by, apply_policy, applied_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                bundle.idempotency_key(),
                bundle.canonical_json(),
                authorization.actor(),
                authorization.policy(),
                now
            ],
        )?;
        let raw_bundle_id = transaction.last_insert_rowid();
        let bundle_id = u64::try_from(raw_bundle_id).map_err(|_| {
            RepositoryError::InvalidDatabase("evidence bundle ID exceeds u64".to_owned())
        })?;
        let mut node_ids = BTreeMap::new();
        for item in bundle.items() {
            let metadata = serde_json::to_string(&serde_json::json!({
                "memory3d_evidence_id": item.id(),
                "source": item.source(),
                "producer": item.producer(),
                "scope": item.scope(),
                "lifecycle": item.lifecycle().as_str()
            }))
            .map_err(|error| {
                RepositoryError::InvalidDatabase(format!(
                    "evidence projection metadata serialization failed: {error}"
                ))
            })?;
            transaction.execute(
                "INSERT INTO nodes (kind, text, metadata, importance, created_at_ms, updated_at_ms, x, y, z) VALUES (?1, ?2, ?3, 0.5, ?4, ?4, NULL, NULL, NULL)",
                params![item.kind().as_str(), item.text(), metadata, now],
            )?;
            let node_id = id_from_i64(transaction.last_insert_rowid())?;
            index_node_terms(&transaction, node_id, item.kind().as_str(), item.text())?;
            let decision = item.decision();
            let scope_json = serde_json::to_string(item.scope()).map_err(|error| {
                RepositoryError::InvalidDatabase(format!(
                    "evidence scope serialization failed: {error}"
                ))
            })?;
            transaction.execute(
                "INSERT INTO evidence_items (evidence_id, bundle_id, node_id, source_identity, producer_identity, scope_json, lifecycle, evidence_created_at_ms, observed_at_ms, conflict_group, decision_actor, decision_policy, decided_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    item.id(),
                    raw_bundle_id,
                    id_to_i64(node_id)?,
                    item.source(),
                    item.producer(),
                    scope_json,
                    item.lifecycle().as_str(),
                    item.created_at_ms(),
                    item.observed_at_ms(),
                    item.conflict_group(),
                    decision.map(DecisionProvenance::actor),
                    decision.map(DecisionProvenance::policy),
                    decision.map(DecisionProvenance::decided_at_ms)
                ],
            )?;
            node_ids.insert(item.id().to_owned(), node_id);
        }

        for item in bundle.items() {
            for (relation, table, target_column, references) in persisted_references(item) {
                for target in references {
                    transaction.execute(
                        &format!(
                            "INSERT INTO {table} (evidence_id, {target_column}) VALUES (?1, ?2)"
                        ),
                        params![item.id(), target],
                    )?;
                    let source_node = node_ids.get(item.id()).copied().ok_or_else(|| {
                        RepositoryError::InvalidDatabase(format!(
                            "evidence node mapping for {:?} disappeared",
                            item.id()
                        ))
                    })?;
                    let target_node = node_ids
                        .get(target)
                        .copied()
                        .map_or_else(|| evidence_node_id_in(&transaction, target), Ok)?;
                    transaction.execute(
                        "INSERT INTO relations (source_id, target_id, name, weight, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, 1.0, ?4, ?4)",
                        params![id_to_i64(source_node)?, id_to_i64(target_node)?, relation, now],
                    )?;
                }
            }
        }
        transaction.commit()?;
        Ok(BundleApplyReport {
            bundle_id,
            idempotency_key: bundle.idempotency_key().to_owned(),
            replayed: false,
            items: node_ids
                .into_iter()
                .map(|(evidence_id, node_id)| AppliedEvidenceItem {
                    evidence_id,
                    node_id,
                })
                .collect(),
            applied_at_ms: now,
        })
    }

    /// Gets one active immutable evidence record by external ID.
    ///
    /// # Errors
    ///
    /// Returns a database or stored-data validation failure.
    pub fn get_evidence_item(
        &self,
        evidence_id: &str,
    ) -> Result<Option<StoredEvidence>, RepositoryError> {
        let evidence_id =
            validate_text(evidence_id.to_owned(), "evidence_id", MAX_EVIDENCE_ID_BYTES)?;
        let raw = self
            .connection
            .query_row(
                "SELECT e.node_id, e.source_identity, e.producer_identity, e.scope_json, e.lifecycle, e.evidence_created_at_ms, e.observed_at_ms, e.conflict_group, e.decision_actor, e.decision_policy, e.decided_at_ms, b.idempotency_key, b.applied_at_ms FROM evidence_items e JOIN evidence_bundles b ON b.id = e.bundle_id WHERE e.evidence_id = ?1 AND b.rolled_back_at_ms IS NULL",
                [&evidence_id],
                read_stored_evidence_row,
            )
            .optional()?;
        raw.map(|value| self.decode_stored_evidence(evidence_id, value))
            .transpose()
    }

    /// Assembles bounded relevant context with explicit status, exclusions, paths, and work.
    ///
    /// # Errors
    ///
    /// Returns a query/options validation, stored-data, or database failure.
    pub fn assemble_evidence_context(
        &self,
        query: &str,
        options: &EvidenceContextOptions,
    ) -> Result<ContextPackage, RepositoryError> {
        options.validate()?;
        let activation = self.activate(query, &options.activation)?;
        let mut admitted = Vec::new();
        let mut excluded = Vec::new();
        let mut estimated_text_bytes = 0_usize;
        let mut candidate_work = 0_usize;
        let mut ordinary_candidates_skipped = 0_usize;
        let mut evidence_candidates_evaluated = 0_usize;
        let ranked_candidates = activation.ranked_candidates();
        let mut stop_reason = None;

        for result in ranked_candidates.iter().take(options.candidate_scan_limit) {
            candidate_work += 1;
            let Some(stored) = self.get_evidence_by_node(result.node.id())? else {
                ordinary_candidates_skipped += 1;
                continue;
            };
            evidence_candidates_evaluated += 1;
            let conflicts = self.visible_conflicts(stored.evidence.id())?;
            let superseded_by = self.superseded_by(stored.evidence.id())?;
            let mut exclusion = if stored.evidence.scope() != &options.scope {
                Some(ContextExclusionReason::ScopeMismatch)
            } else if !conflicts.is_empty() {
                Some(ContextExclusionReason::Conflict)
            } else if !superseded_by.is_empty() {
                Some(ContextExclusionReason::Superseded)
            } else if !options
                .accepted_lifecycles
                .contains(&stored.evidence.lifecycle())
            {
                Some(ContextExclusionReason::Lifecycle)
            } else if result.score < options.min_relevance_score {
                Some(ContextExclusionReason::LowRelevance)
            } else {
                None
            };
            if exclusion.is_none()
                && estimated_text_bytes.saturating_add(stored.evidence.text().len())
                    > options.max_bytes
            {
                exclusion = Some(ContextExclusionReason::SizeLimit);
            }
            let context_item = EvidenceContextItem {
                stored,
                relevance_score: result.score,
                path: result.path.clone(),
                conflicts,
                superseded_by,
                selection_provenance: "bounded_lexical_seed_and_graph_activation",
                exclusion: exclusion.clone(),
            };
            if exclusion.is_some() {
                excluded.push(context_item);
            } else {
                estimated_text_bytes =
                    estimated_text_bytes.saturating_add(context_item.stored.evidence.text().len());
                admitted.push(context_item);
            }
            if admitted.len() == options.result_limit {
                stop_reason = Some(ContextStopReason::ResultLimitReached);
                break;
            }
            if estimated_text_bytes == options.max_bytes {
                stop_reason = Some(ContextStopReason::ByteLimitReached);
                break;
            }
        }
        let stop_reason = stop_reason.unwrap_or({
            if candidate_work == options.candidate_scan_limit
                && ranked_candidates.len() > candidate_work
            {
                ContextStopReason::CandidateScanLimitReached
            } else {
                ContextStopReason::CandidateStreamExhausted
            }
        });
        Ok(ContextPackage {
            query: query.to_owned(),
            scope: options.scope.clone(),
            abstained: admitted.is_empty(),
            admitted,
            excluded,
            estimated_text_bytes,
            min_relevance_score: options.min_relevance_score,
            candidate_work,
            ordinary_candidates_skipped,
            evidence_candidates_evaluated,
            candidate_scan_limit: options.candidate_scan_limit,
            result_limit: options.result_limit,
            stop_reason,
            traversal: activation.stats,
            database: activation.database,
        })
    }

    /// Explicitly compensates one active bundle while retaining its idempotency ledger.
    ///
    /// Rollback is blocked when later evidence or an ordinary graph relation depends on one of
    /// the bundle's projected nodes. Nothing is removed on a blocked or failed transaction.
    ///
    /// # Errors
    ///
    /// Returns a missing, already-rolled-back, dependency, validation, or database failure.
    pub fn rollback_evidence_bundle(
        &mut self,
        idempotency_key: &str,
    ) -> Result<BundleRollbackReport, RepositoryError> {
        let key = validate_text(
            idempotency_key.to_owned(),
            "idempotency_key",
            MAX_EVIDENCE_ID_BYTES,
        )?;
        let ledger = self
            .bundle_ledger(&key)?
            .ok_or_else(|| RepositoryError::EvidenceBundleNotFound(key.clone()))?;
        if ledger.rolled_back_at_ms.is_some() {
            return Err(RepositoryError::EvidenceBundleRolledBack(key));
        }
        if let Some(dependency) = self.external_evidence_dependency(ledger.id)? {
            return Err(RepositoryError::EvidenceRollbackBlocked {
                idempotency_key: key,
                referenced_by: dependency,
            });
        }
        if let Some(dependency) = self.external_graph_dependency(ledger.id)? {
            return Err(RepositoryError::EvidenceRollbackBlocked {
                idempotency_key: key,
                referenced_by: dependency,
            });
        }

        let now = unix_time_ms()?;
        let transaction = self.connection.transaction()?;
        let node_ids = bundle_node_ids(&transaction, ledger.id)?;
        for table in [
            "evidence_derivations",
            "evidence_conflicts",
            "evidence_supersessions",
            "evidence_decision_support",
        ] {
            transaction.execute(
                &format!(
                    "DELETE FROM {table} WHERE evidence_id IN (SELECT evidence_id FROM evidence_items WHERE bundle_id = ?1)"
                ),
                [ledger.id],
            )?;
        }
        for node_id in &node_ids {
            transaction.execute(
                "DELETE FROM relations WHERE source_id = ?1 OR target_id = ?1",
                [id_to_i64(*node_id)?],
            )?;
        }
        let removed_items = transaction.execute(
            "DELETE FROM evidence_items WHERE bundle_id = ?1",
            [ledger.id],
        )?;
        for node_id in &node_ids {
            transaction.execute("DELETE FROM nodes WHERE id = ?1", [id_to_i64(*node_id)?])?;
        }
        transaction.execute(
            "UPDATE evidence_bundles SET rolled_back_at_ms = ?1 WHERE id = ?2 AND rolled_back_at_ms IS NULL",
            params![now, ledger.id],
        )?;
        transaction.commit()?;
        Ok(BundleRollbackReport {
            idempotency_key: key,
            removed_items,
            removed_nodes: node_ids.len(),
            rolled_back_at_ms: now,
        })
    }

    fn active_evidence_exists(&self, evidence_id: &str) -> Result<bool, RepositoryError> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM evidence_items e JOIN evidence_bundles b ON b.id = e.bundle_id WHERE e.evidence_id = ?1 AND b.rolled_back_at_ms IS NULL)",
                [evidence_id],
                |row| row.get(0),
            )
            .map_err(RepositoryError::from)
    }

    fn active_conflict_group(
        &self,
        evidence_id: &str,
    ) -> Result<(bool, Option<String>), RepositoryError> {
        let group = self
            .connection
            .query_row(
                "SELECT e.conflict_group FROM evidence_items e JOIN evidence_bundles b ON b.id = e.bundle_id WHERE e.evidence_id = ?1 AND b.rolled_back_at_ms IS NULL",
                [evidence_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(RepositoryError::from)?;
        Ok(group.map_or((false, None), |value| (true, value)))
    }

    fn bundle_ledger(&self, key: &str) -> Result<Option<BundleLedger>, RepositoryError> {
        self.connection
            .query_row(
                "SELECT id, canonical_payload, applied_at_ms, rolled_back_at_ms FROM evidence_bundles WHERE idempotency_key = ?1",
                [key],
                |row| {
                    Ok(BundleLedger {
                        id: row.get(0)?,
                        canonical_payload: row.get(1)?,
                        applied_at_ms: row.get(2)?,
                        rolled_back_at_ms: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(RepositoryError::from)
    }

    fn load_apply_report(
        &self,
        key: &str,
        replayed: bool,
    ) -> Result<BundleApplyReport, RepositoryError> {
        let ledger = self
            .bundle_ledger(key)?
            .ok_or_else(|| RepositoryError::EvidenceBundleNotFound(key.to_owned()))?;
        if ledger.rolled_back_at_ms.is_some() {
            return Err(RepositoryError::EvidenceBundleRolledBack(key.to_owned()));
        }
        let mut statement = self.connection.prepare(
            "SELECT evidence_id, node_id FROM evidence_items WHERE bundle_id = ?1 ORDER BY evidence_id",
        )?;
        let rows = statement.query_map([ledger.id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let items = rows
            .map(|row| {
                let (evidence_id, node_id) = row?;
                Ok(AppliedEvidenceItem {
                    evidence_id,
                    node_id: id_from_i64(node_id)?,
                })
            })
            .collect::<Result<Vec<_>, RepositoryError>>()?;
        Ok(BundleApplyReport {
            bundle_id: u64::try_from(ledger.id).map_err(|_| {
                RepositoryError::InvalidDatabase("evidence bundle ID is invalid".to_owned())
            })?,
            idempotency_key: key.to_owned(),
            replayed,
            items,
            applied_at_ms: ledger.applied_at_ms,
        })
    }

    fn decode_stored_evidence(
        &self,
        evidence_id: String,
        raw: RawStoredEvidence,
    ) -> Result<StoredEvidence, RepositoryError> {
        let node_id = id_from_i64(raw.node_id)?;
        let node = self.get_node(node_id)?.ok_or_else(|| {
            RepositoryError::InvalidDatabase(format!(
                "evidence {evidence_id:?} references missing node {node_id}"
            ))
        })?;
        let scope =
            serde_json::from_str::<BTreeMap<String, String>>(&raw.scope_json).map_err(|error| {
                RepositoryError::InvalidDatabase(format!(
                    "evidence {evidence_id:?} has invalid scope: {error}"
                ))
            })?;
        validate_scope(&scope, "stored.scope")?;
        let lifecycle = EvidenceLifecycle::from_stored(&raw.lifecycle)?;
        let derives_from = self.reference_targets(
            "evidence_derivations",
            "supporting_evidence_id",
            &evidence_id,
        )?;
        let conflicts_with = self.reference_targets(
            "evidence_conflicts",
            "conflicting_evidence_id",
            &evidence_id,
        )?;
        let supersedes = self.reference_targets(
            "evidence_supersessions",
            "superseded_evidence_id",
            &evidence_id,
        )?;
        let supporting_evidence_ids = self.reference_targets(
            "evidence_decision_support",
            "supporting_evidence_id",
            &evidence_id,
        )?;
        let decision = match (raw.decision_actor, raw.decision_policy, raw.decided_at_ms) {
            (None, None, None) => None,
            (Some(actor), Some(policy), Some(decided_at_ms)) => Some(DecisionProvenance {
                actor,
                policy,
                supporting_evidence_ids,
                decided_at_ms,
            }),
            _ => {
                return Err(RepositoryError::InvalidDatabase(format!(
                    "evidence {evidence_id:?} has partial decision provenance"
                )));
            }
        };
        Ok(StoredEvidence {
            evidence: EvidenceItem {
                id: evidence_id,
                kind: node.kind().clone(),
                text: node.text().to_owned(),
                source: raw.source,
                producer: raw.producer,
                scope,
                lifecycle,
                created_at_ms: raw.evidence_created_at_ms,
                observed_at_ms: raw.observed_at_ms,
                derives_from,
                conflict_group: raw.conflict_group,
                conflicts_with,
                supersedes,
                decision,
            },
            node,
            bundle_key: raw.bundle_key,
            applied_at_ms: raw.applied_at_ms,
        })
    }

    fn reference_targets(
        &self,
        table: &str,
        target_column: &str,
        evidence_id: &str,
    ) -> Result<Vec<String>, RepositoryError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {target_column} FROM {table} WHERE evidence_id = ?1 ORDER BY {target_column}"
        ))?;
        let rows = statement.query_map([evidence_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(RepositoryError::from)
    }

    fn get_evidence_by_node(
        &self,
        node_id: NodeId,
    ) -> Result<Option<StoredEvidence>, RepositoryError> {
        let evidence_id = self
            .connection
            .query_row(
                "SELECT e.evidence_id FROM evidence_items e JOIN evidence_bundles b ON b.id = e.bundle_id WHERE e.node_id = ?1 AND b.rolled_back_at_ms IS NULL",
                [id_to_i64(node_id)?],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        evidence_id
            .map(|id| self.get_evidence_item(&id))
            .transpose()
            .map(Option::flatten)
    }

    fn visible_conflicts(&self, evidence_id: &str) -> Result<Vec<String>, RepositoryError> {
        let mut statement = self.connection.prepare(
            "SELECT conflicting_evidence_id FROM evidence_conflicts WHERE evidence_id = ?1 UNION SELECT evidence_id FROM evidence_conflicts WHERE conflicting_evidence_id = ?1 ORDER BY 1",
        )?;
        let rows = statement.query_map([evidence_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(RepositoryError::from)
    }

    fn superseded_by(&self, evidence_id: &str) -> Result<Vec<String>, RepositoryError> {
        let mut statement = self.connection.prepare(
            "SELECT evidence_id FROM evidence_supersessions WHERE superseded_evidence_id = ?1 ORDER BY evidence_id",
        )?;
        let rows = statement.query_map([evidence_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(RepositoryError::from)
    }

    fn external_evidence_dependency(
        &self,
        bundle_id: i64,
    ) -> Result<Option<String>, RepositoryError> {
        for (table, target) in [
            ("evidence_derivations", "supporting_evidence_id"),
            ("evidence_conflicts", "conflicting_evidence_id"),
            ("evidence_supersessions", "superseded_evidence_id"),
            ("evidence_decision_support", "supporting_evidence_id"),
        ] {
            let sql = format!(
                "SELECT r.evidence_id FROM {table} r JOIN evidence_items source ON source.evidence_id = r.evidence_id JOIN evidence_items target ON target.evidence_id = r.{target} WHERE target.bundle_id = ?1 AND source.bundle_id <> ?1 ORDER BY r.evidence_id LIMIT 1"
            );
            if let Some(id) = self
                .connection
                .query_row(&sql, [bundle_id], |row| row.get::<_, String>(0))
                .optional()?
            {
                return Ok(Some(format!("later evidence {id:?}")));
            }
        }
        Ok(None)
    }

    fn external_graph_dependency(&self, bundle_id: i64) -> Result<Option<String>, RepositoryError> {
        self.connection
            .query_row(
                "SELECT printf('graph relation %d -[%s]-> %d', r.source_id, r.name, r.target_id) FROM relations r WHERE ((r.source_id IN (SELECT node_id FROM evidence_items WHERE bundle_id = ?1)) <> (r.target_id IN (SELECT node_id FROM evidence_items WHERE bundle_id = ?1))) ORDER BY r.source_id, r.target_id, r.name LIMIT 1",
                [bundle_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(RepositoryError::from)
    }
}

fn bundle_preview(bundle: &EvidenceBundle, status: BundlePreviewStatus) -> BundlePreview {
    BundlePreview {
        status,
        idempotency_key: bundle.idempotency_key().to_owned(),
        fingerprint: bundle.fingerprint(),
        evidence_ids: bundle
            .items()
            .iter()
            .map(|item| item.id().to_owned())
            .collect(),
        projected_nodes: bundle.items().len(),
        projected_relations: bundle
            .items()
            .iter()
            .map(|item| {
                item.derives_from().len()
                    + item.conflicts_with().len()
                    + item.supersedes().len()
                    + item
                        .decision()
                        .map_or(0, |decision| decision.supporting_evidence_ids().len())
            })
            .sum(),
    }
}

fn evidence_references(item: &EvidenceItem) -> Vec<(&'static str, &[String])> {
    let mut references = vec![
        ("derives_from", item.derives_from()),
        ("conflicts_with", item.conflicts_with()),
        ("supersedes", item.supersedes()),
    ];
    if let Some(decision) = item.decision() {
        references.push(("decision_support", decision.supporting_evidence_ids()));
    }
    references
}

type PersistedReferences<'a> = (&'static str, &'static str, &'static str, &'a [String]);

fn persisted_references(item: &EvidenceItem) -> Vec<PersistedReferences<'_>> {
    let mut references = vec![
        (
            "evidence:derived_from",
            "evidence_derivations",
            "supporting_evidence_id",
            item.derives_from(),
        ),
        (
            "evidence:conflicts_with",
            "evidence_conflicts",
            "conflicting_evidence_id",
            item.conflicts_with(),
        ),
        (
            "evidence:supersedes",
            "evidence_supersessions",
            "superseded_evidence_id",
            item.supersedes(),
        ),
    ];
    if let Some(decision) = item.decision() {
        references.push((
            "evidence:decision_support",
            "evidence_decision_support",
            "supporting_evidence_id",
            decision.supporting_evidence_ids(),
        ));
    }
    references
}

fn evidence_node_id_in(
    transaction: &Transaction<'_>,
    evidence_id: &str,
) -> Result<NodeId, RepositoryError> {
    let node_id = transaction
        .query_row(
            "SELECT node_id FROM evidence_items WHERE evidence_id = ?1",
            [evidence_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .ok_or_else(|| RepositoryError::EvidenceReferenceNotFound {
            evidence_id: evidence_id.to_owned(),
            relation: "stored_reference",
            target: evidence_id.to_owned(),
        })?;
    id_from_i64(node_id)
}

#[derive(Debug)]
struct BundleLedger {
    id: i64,
    canonical_payload: String,
    applied_at_ms: i64,
    rolled_back_at_ms: Option<i64>,
}

#[derive(Debug)]
struct RawStoredEvidence {
    node_id: i64,
    source: String,
    producer: String,
    scope_json: String,
    lifecycle: String,
    evidence_created_at_ms: i64,
    observed_at_ms: Option<i64>,
    conflict_group: Option<String>,
    decision_actor: Option<String>,
    decision_policy: Option<String>,
    decided_at_ms: Option<i64>,
    bundle_key: String,
    applied_at_ms: i64,
}

fn read_stored_evidence_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawStoredEvidence> {
    Ok(RawStoredEvidence {
        node_id: row.get(0)?,
        source: row.get(1)?,
        producer: row.get(2)?,
        scope_json: row.get(3)?,
        lifecycle: row.get(4)?,
        evidence_created_at_ms: row.get(5)?,
        observed_at_ms: row.get(6)?,
        conflict_group: row.get(7)?,
        decision_actor: row.get(8)?,
        decision_policy: row.get(9)?,
        decided_at_ms: row.get(10)?,
        bundle_key: row.get(11)?,
        applied_at_ms: row.get(12)?,
    })
}

fn bundle_node_ids(
    transaction: &Transaction<'_>,
    bundle_id: i64,
) -> Result<Vec<NodeId>, RepositoryError> {
    let mut statement = transaction
        .prepare("SELECT node_id FROM evidence_items WHERE bundle_id = ?1 ORDER BY node_id")?;
    let rows = statement.query_map([bundle_id], |row| row.get::<_, i64>(0))?;
    rows.map(|row| id_from_i64(row?)).collect()
}
