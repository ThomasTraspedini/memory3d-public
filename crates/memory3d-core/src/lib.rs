//! Validated domain types and durable storage for every `Memory3D` adapter.

#![forbid(unsafe_code)]

use std::{error::Error, fmt, num::NonZeroU128};

mod community;
mod evidence;
mod repository;
mod retrieval;

pub use community::{
    CommunityBuildReport, CommunityMembership, CommunityPolicy, CommunityResolution,
    CommunityResolutionSummary, CommunityStatus,
};
pub use evidence::{
    AppliedEvidenceItem, ApplyAuthorization, BundleApplyReport, BundlePreview, BundlePreviewStatus,
    BundleRollbackReport, ContextExclusionReason, ContextPackage, ContextStopReason,
    DEFAULT_EVIDENCE_MIN_RELEVANCE, DecisionProvenance, EvidenceBundle, EvidenceContextItem,
    EvidenceContextOptions, EvidenceError, EvidenceItem, EvidenceLifecycle, MAX_BUNDLE_BYTES,
    MAX_BUNDLE_ITEMS, MAX_CONTEXT_BYTES, MAX_EVIDENCE_CANDIDATE_SCAN_LIMIT, MAX_EVIDENCE_ID_BYTES,
    MAX_EVIDENCE_REFERENCES, MAX_IDENTITY_BYTES, MAX_SCOPE_ENTRIES, StoredEvidence,
};
pub use repository::{IntegrityReport, Repository, RepositoryError};
pub use retrieval::{
    ActivationReport, ActivationTimings, AdjacencyInspection, CommunityDiagnostics,
    CommunityEdgeDiagnostic, CommunityPathProvenance, CommunityRoute, DatabaseStats,
    FeedbackPolicy, SearchOptions, SearchResult, TraversalStats,
};

/// Maximum UTF-8 byte length accepted for memory text.
pub const MAX_TEXT_BYTES: usize = 1_048_576;
/// Maximum UTF-8 byte length accepted for a memory kind.
pub const MAX_KIND_BYTES: usize = 64;
/// Maximum UTF-8 byte length accepted for a relation name.
pub const MAX_RELATION_NAME_BYTES: usize = 64;
/// Maximum encoded JSON byte length accepted for memory metadata.
pub const MAX_METADATA_BYTES: usize = 65_536;
/// Maximum UTF-8 byte length accepted for a lexical query.
pub const MAX_QUERY_BYTES: usize = 4_096;
/// Maximum number of distinct normalized terms accepted in a lexical query.
pub const MAX_QUERY_TERMS: usize = 64;
/// Maximum number of graph hops in an activation request.
pub const MAX_HOPS: u8 = 16;
/// Maximum number of lexical seeds in an activation request.
pub const MAX_SEEDS: usize = 32;
/// Maximum number of returned activation results.
pub const MAX_RESULTS: usize = 100;
/// Maximum number of nodes an activation may visit.
pub const MAX_VISITED_NODES: usize = 10_000;
/// Maximum number of edges an activation may examine.
pub const MAX_VISITED_EDGES: usize = 50_000;
/// Inclusive absolute bound for each coordinate component in the documented local frame.
pub const MAX_COORDINATE_ABS: f64 = 1_000_000.0;
const MAX_COORDINATE_ABS_F32: f32 = 1_000_000.0;

/// Identifies which public input failed validation.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationError {
    /// A required string was empty or whitespace-only.
    Empty(&'static str),
    /// An identifier was zero, which is reserved as invalid.
    ZeroId,
    /// A feedback-event identifier was zero, which is reserved as invalid.
    ZeroFeedbackEventId,
    /// A string exceeded its UTF-8 byte limit.
    TooLong {
        /// Input field name.
        field: &'static str,
        /// Maximum permitted byte count.
        max_bytes: usize,
    },
    /// A floating-point value was not finite or outside its inclusive range.
    InvalidNumber {
        /// Input field name.
        field: &'static str,
        /// Inclusive minimum.
        min: f32,
        /// Inclusive maximum.
        max: f32,
    },
    /// An integer was outside its inclusive range.
    OutOfRange {
        /// Input field name.
        field: &'static str,
        /// Inclusive minimum.
        min: usize,
        /// Inclusive maximum.
        max: usize,
    },
    /// Metadata was not a JSON object.
    MetadataNotObject,
    /// An activation path does not terminate at its result node.
    PathEndpointMismatch,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(field) => write!(formatter, "{field} must not be empty"),
            Self::ZeroId => formatter.write_str("node ID must be non-zero"),
            Self::ZeroFeedbackEventId => formatter.write_str("feedback event ID must be non-zero"),
            Self::TooLong { field, max_bytes } => {
                write!(formatter, "{field} must be at most {max_bytes} bytes")
            }
            Self::InvalidNumber { field, min, max } => write!(
                formatter,
                "{field} must be finite and between {min} and {max} inclusive"
            ),
            Self::OutOfRange { field, min, max } => {
                write!(
                    formatter,
                    "{field} must be between {min} and {max} inclusive"
                )
            }
            Self::MetadataNotObject => formatter.write_str("metadata must be a JSON object"),
            Self::PathEndpointMismatch => {
                formatter.write_str("activation path must end at the result node")
            }
        }
    }
}

/// Stable, opaque identifier for an explicit feedback event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FeedbackEventId(NonZeroU128);

impl FeedbackEventId {
    /// Creates an ID, rejecting the reserved zero value.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::ZeroFeedbackEventId`] when `value` is zero.
    pub fn new(value: u128) -> Result<Self, ValidationError> {
        NonZeroU128::new(value).map_or(Err(ValidationError::ZeroFeedbackEventId), |id| Ok(Self(id)))
    }

    /// Returns the numeric representation used for durable round trips.
    #[must_use]
    pub const fn get(self) -> u128 {
        self.0.get()
    }
}

impl fmt::Display for FeedbackEventId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// JSON object attached to a memory node.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Metadata(serde_json::Map<String, serde_json::Value>);

impl Metadata {
    /// Validates an object and its encoded size.
    ///
    /// # Errors
    ///
    /// Returns a validation error for non-object JSON or an object larger than
    /// [`MAX_METADATA_BYTES`].
    pub fn new(value: serde_json::Value) -> Result<Self, ValidationError> {
        let serde_json::Value::Object(object) = value else {
            return Err(ValidationError::MetadataNotObject);
        };
        let encoded = serde_json::to_vec(&object).map_err(|_| ValidationError::TooLong {
            field: "metadata",
            max_bytes: MAX_METADATA_BYTES,
        })?;
        if encoded.len() > MAX_METADATA_BYTES {
            return Err(ValidationError::TooLong {
                field: "metadata",
                max_bytes: MAX_METADATA_BYTES,
            });
        }
        Ok(Self(object))
    }

    /// Returns the metadata object.
    #[must_use]
    pub const fn as_object(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.0
    }

    pub(crate) fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.0)
    }
}

/// Optional three-dimensional coordinates in a caller-defined local Cartesian frame.
///
/// All coordinates in one database must use the same caller-defined frame, origin, and scale. The
/// core validates numeric shape and range; it cannot infer or verify frame identity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coordinates {
    x: f64,
    y: f64,
    z: f64,
}

impl Coordinates {
    /// Creates finite coordinates within the supported local-frame range.
    ///
    /// # Errors
    ///
    /// Returns a validation error if any component is not finite or its absolute value exceeds
    /// [`MAX_COORDINATE_ABS`].
    pub fn new(x: f64, y: f64, z: f64) -> Result<Self, ValidationError> {
        for (field, value) in [("x", x), ("y", y), ("z", z)] {
            if !value.is_finite() || !(-MAX_COORDINATE_ABS..=MAX_COORDINATE_ABS).contains(&value) {
                return Err(ValidationError::InvalidNumber {
                    field,
                    min: -MAX_COORDINATE_ABS_F32,
                    max: MAX_COORDINATE_ABS_F32,
                });
            }
        }
        Ok(Self { x, y, z })
    }

    /// Returns the x coordinate.
    #[must_use]
    pub const fn x(self) -> f64 {
        self.x
    }
    /// Returns the y coordinate.
    #[must_use]
    pub const fn y(self) -> f64 {
        self.y
    }
    /// Returns the z coordinate.
    #[must_use]
    pub const fn z(self) -> f64 {
        self.z
    }
}

impl Error for ValidationError {}

/// Stable, opaque identifier for a memory node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(NonZeroU128);

impl NodeId {
    /// Creates an ID, rejecting the reserved zero value.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::ZeroId`] when `value` is zero.
    pub fn new(value: u128) -> Result<Self, ValidationError> {
        NonZeroU128::new(value).map_or(Err(ValidationError::ZeroId), |id| Ok(Self(id)))
    }

    /// Returns the numeric representation used for durable round trips.
    #[must_use]
    pub const fn get(self) -> u128 {
        self.0.get()
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Extensible, validated memory classification.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemoryKind(String);

impl MemoryKind {
    /// Creates a non-empty memory kind.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the kind is blank or exceeds [`MAX_KIND_BYTES`].
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        validate_string(value.into(), "kind", MAX_KIND_BYTES).map(Self)
    }

    /// Returns the kind as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A stored text memory, independent of any persistence representation.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryNode {
    id: NodeId,
    kind: MemoryKind,
    text: String,
    importance: f32,
    metadata: Metadata,
    coordinates: Option<Coordinates>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

impl MemoryNode {
    /// Creates a validated memory node.
    ///
    /// # Errors
    ///
    /// Returns a validation error when text is blank or too long, or importance is outside
    /// the finite inclusive range `[0, 1]`.
    pub fn new(
        id: NodeId,
        kind: MemoryKind,
        text: impl Into<String>,
        importance: f32,
    ) -> Result<Self, ValidationError> {
        let text = validate_string(text.into(), "text", MAX_TEXT_BYTES)?;
        validate_unit_interval(importance, "importance")?;
        Ok(Self {
            id,
            kind,
            text,
            importance,
            metadata: Metadata::default(),
            coordinates: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        })
    }

    pub(crate) fn from_stored(
        id: NodeId,
        new: NewMemory,
        created_at_ms: i64,
        updated_at_ms: i64,
    ) -> Self {
        Self {
            id,
            kind: new.kind,
            text: new.text,
            importance: new.importance,
            metadata: new.metadata,
            coordinates: new.coordinates,
            created_at_ms,
            updated_at_ms,
        }
    }

    /// Returns the stable node identifier.
    #[must_use]
    pub const fn id(&self) -> NodeId {
        self.id
    }
    /// Returns the memory kind.
    #[must_use]
    pub const fn kind(&self) -> &MemoryKind {
        &self.kind
    }
    /// Returns the stored text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Returns the importance in the inclusive range `[0, 1]`.
    #[must_use]
    pub const fn importance(&self) -> f32 {
        self.importance
    }
    /// Returns the stored JSON metadata object.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    /// Returns optional coordinates.
    #[must_use]
    pub const fn coordinates(&self) -> Option<Coordinates> {
        self.coordinates
    }
    /// Returns the creation time as Unix epoch milliseconds.
    #[must_use]
    pub const fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }
    /// Returns the last-update time as Unix epoch milliseconds.
    #[must_use]
    pub const fn updated_at_ms(&self) -> i64 {
        self.updated_at_ms
    }
}

/// Validated input for creating a memory node.
#[derive(Debug, Clone, PartialEq)]
pub struct NewMemory {
    pub(crate) kind: MemoryKind,
    pub(crate) text: String,
    pub(crate) importance: f32,
    pub(crate) metadata: Metadata,
    pub(crate) coordinates: Option<Coordinates>,
}

impl NewMemory {
    /// Creates a validated memory input with empty metadata and no coordinates.
    ///
    /// # Errors
    ///
    /// Returns a validation error for invalid text or importance.
    pub fn new(
        kind: MemoryKind,
        text: impl Into<String>,
        importance: f32,
    ) -> Result<Self, ValidationError> {
        let text = validate_string(text.into(), "text", MAX_TEXT_BYTES)?;
        validate_unit_interval(importance, "importance")?;
        Ok(Self {
            kind,
            text,
            importance,
            metadata: Metadata::default(),
            coordinates: None,
        })
    }

    /// Attaches validated metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Attaches optional coordinates.
    #[must_use]
    pub fn with_coordinates(mut self, coordinates: Coordinates) -> Self {
        self.coordinates = Some(coordinates);
        self
    }
}

/// A validated directed relation between two existing node IDs.
#[derive(Debug, Clone, PartialEq)]
pub struct Relation {
    source: NodeId,
    target: NodeId,
    name: String,
    weight: f32,
    created_at_ms: i64,
    updated_at_ms: i64,
    reinforcement_count: u64,
}

impl Relation {
    /// Creates a directed relation with a finite weight in `[0, 1]`.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the name is blank or too long, or weight is outside
    /// the finite inclusive range `[0, 1]`.
    pub fn new(
        source: NodeId,
        target: NodeId,
        name: impl Into<String>,
        weight: f32,
    ) -> Result<Self, ValidationError> {
        let name = validate_string(name.into(), "relation name", MAX_RELATION_NAME_BYTES)?;
        validate_unit_interval(weight, "weight")?;
        Ok(Self {
            source,
            target,
            name,
            weight,
            created_at_ms: 0,
            updated_at_ms: 0,
            reinforcement_count: 0,
        })
    }

    pub(crate) fn from_stored(
        new: NewRelation,
        created_at_ms: i64,
        updated_at_ms: i64,
        reinforcement_count: u64,
    ) -> Self {
        Self {
            source: new.source,
            target: new.target,
            name: new.name,
            weight: new.weight,
            created_at_ms,
            updated_at_ms,
            reinforcement_count,
        }
    }

    /// Returns the source node.
    #[must_use]
    pub const fn source(&self) -> NodeId {
        self.source
    }
    /// Returns the target node.
    #[must_use]
    pub const fn target(&self) -> NodeId {
        self.target
    }
    /// Returns the relation name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Returns the relation weight.
    #[must_use]
    pub const fn weight(&self) -> f32 {
        self.weight
    }
    /// Returns the creation time as Unix epoch milliseconds.
    #[must_use]
    pub const fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }
    /// Returns the last-update time as Unix epoch milliseconds.
    #[must_use]
    pub const fn updated_at_ms(&self) -> i64 {
        self.updated_at_ms
    }
    /// Returns the informational reinforcement count.
    #[must_use]
    pub const fn reinforcement_count(&self) -> u64 {
        self.reinforcement_count
    }
}

/// Polarity for an explicit operator feedback event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackKind {
    /// Increase future feedback-aware activation through the relation.
    Positive,
    /// Decrease future feedback-aware activation through the relation.
    Negative,
}

impl FeedbackKind {
    /// Returns the stable storage/report label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
        }
    }

    pub(crate) fn from_str(value: &str) -> Result<Self, ValidationError> {
        match value {
            "positive" => Ok(Self::Positive),
            "negative" => Ok(Self::Negative),
            _ => Err(ValidationError::Empty("feedback kind")),
        }
    }
}

/// Validated input for recording explicit feedback on one existing relation.
#[derive(Debug, Clone, PartialEq)]
pub struct NewFeedbackEvent {
    pub(crate) source: NodeId,
    pub(crate) target: NodeId,
    pub(crate) relation_name: String,
    pub(crate) kind: FeedbackKind,
    pub(crate) strength: f32,
    pub(crate) occurred_at_ms: i64,
}

impl NewFeedbackEvent {
    /// Creates an explicit feedback event for one stored relation.
    ///
    /// # Errors
    ///
    /// Returns a validation error for an invalid relation name, non-finite strength outside
    /// `(0, 1]`, or a negative deterministic event timestamp.
    pub fn new(
        source: NodeId,
        target: NodeId,
        relation_name: impl Into<String>,
        kind: FeedbackKind,
        strength: f32,
        occurred_at_ms: i64,
    ) -> Result<Self, ValidationError> {
        let relation_name = validate_string(
            relation_name.into(),
            "relation name",
            MAX_RELATION_NAME_BYTES,
        )?;
        if !(strength.is_finite() && strength > 0.0 && strength <= 1.0) {
            return Err(ValidationError::InvalidNumber {
                field: "feedback strength",
                min: f32::MIN_POSITIVE,
                max: 1.0,
            });
        }
        if occurred_at_ms < 0 {
            return Err(ValidationError::OutOfRange {
                field: "occurred_at_ms",
                min: 0,
                max: usize::MAX,
            });
        }
        Ok(Self {
            source,
            target,
            relation_name,
            kind,
            strength,
            occurred_at_ms,
        })
    }
}

/// Durable explicit feedback event attached to one relation.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedbackEvent {
    id: FeedbackEventId,
    source: NodeId,
    target: NodeId,
    relation_name: String,
    kind: FeedbackKind,
    strength: f32,
    occurred_at_ms: i64,
    created_at_ms: i64,
}

impl FeedbackEvent {
    pub(crate) fn from_stored(
        id: FeedbackEventId,
        new: NewFeedbackEvent,
        created_at_ms: i64,
    ) -> Self {
        Self {
            id,
            source: new.source,
            target: new.target,
            relation_name: new.relation_name,
            kind: new.kind,
            strength: new.strength,
            occurred_at_ms: new.occurred_at_ms,
            created_at_ms,
        }
    }

    /// Returns the event identifier.
    #[must_use]
    pub const fn id(&self) -> FeedbackEventId {
        self.id
    }
    /// Returns the relation source.
    #[must_use]
    pub const fn source(&self) -> NodeId {
        self.source
    }
    /// Returns the relation target.
    #[must_use]
    pub const fn target(&self) -> NodeId {
        self.target
    }
    /// Returns the relation name.
    #[must_use]
    pub fn relation_name(&self) -> &str {
        &self.relation_name
    }
    /// Returns the feedback polarity.
    #[must_use]
    pub const fn kind(&self) -> FeedbackKind {
        self.kind
    }
    /// Returns the event strength in `(0, 1]`.
    #[must_use]
    pub const fn strength(&self) -> f32 {
        self.strength
    }
    /// Returns the deterministic operator-supplied event timestamp.
    #[must_use]
    pub const fn occurred_at_ms(&self) -> i64 {
        self.occurred_at_ms
    }
    /// Returns the durable insertion timestamp.
    #[must_use]
    pub const fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }
}

/// Validated input for creating a directed relation.
#[derive(Debug, Clone, PartialEq)]
pub struct NewRelation {
    pub(crate) source: NodeId,
    pub(crate) target: NodeId,
    pub(crate) name: String,
    pub(crate) weight: f32,
}

impl NewRelation {
    /// Creates a directed relation input with a finite weight in `[0, 1]`.
    ///
    /// # Errors
    ///
    /// Returns a validation error for an invalid name or weight.
    pub fn new(
        source: NodeId,
        target: NodeId,
        name: impl Into<String>,
        weight: f32,
    ) -> Result<Self, ValidationError> {
        let name = validate_string(name.into(), "relation name", MAX_RELATION_NAME_BYTES)?;
        validate_unit_interval(weight, "weight")?;
        Ok(Self {
            source,
            target,
            name,
            weight,
        })
    }
}

/// Hard bounds for one activation request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationOptions {
    /// Maximum relation depth.
    pub hops: u8,
    /// Maximum number of selected seeds.
    pub seed_limit: usize,
    /// Maximum number of results.
    pub limit: usize,
    /// Maximum distinct nodes visited.
    pub max_visited_nodes: usize,
    /// Maximum relations examined.
    pub max_visited_edges: usize,
    /// Whether seed memories may appear in results.
    pub include_seeds: bool,
}

impl ActivationOptions {
    /// Validates every request bound against the core's hard caps.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::OutOfRange`] for the first bound outside its allowed range.
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_range(usize::from(self.hops), "hops", 0, usize::from(MAX_HOPS))?;
        validate_range(self.seed_limit, "seed_limit", 1, MAX_SEEDS)?;
        validate_range(self.limit, "limit", 1, MAX_RESULTS)?;
        validate_range(
            self.max_visited_nodes,
            "max_visited_nodes",
            1,
            MAX_VISITED_NODES,
        )?;
        validate_range(
            self.max_visited_edges,
            "max_visited_edges",
            1,
            MAX_VISITED_EDGES,
        )
    }
}

impl Default for ActivationOptions {
    fn default() -> Self {
        Self {
            hops: 3,
            seed_limit: 8,
            limit: 10,
            max_visited_nodes: 1_000,
            max_visited_edges: 5_000,
            include_seeds: false,
        }
    }
}

/// One directed relation step in a reconstructable activation path.
#[derive(Debug, Clone, PartialEq)]
pub struct PathStep {
    /// Node from which this step departs.
    pub source: NodeId,
    /// Stored relation name.
    pub relation: String,
    /// Stored relation weight.
    pub weight: f32,
    /// Node reached by this step.
    pub target: NodeId,
}

impl PathStep {
    /// Creates a validated path step.
    #[must_use]
    pub fn new(relation: &Relation) -> Self {
        Self {
            source: relation.source(),
            relation: relation.name().to_owned(),
            weight: relation.weight(),
            target: relation.target(),
        }
    }
}

/// Ordered path from a selected seed to an activated node.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivationPath {
    /// Seed at which activation began.
    pub seed: NodeId,
    /// Ordered relation steps; empty only when the result itself is the seed.
    pub steps: Vec<PathStep>,
}

/// One ranked, explainable activation result.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivationResult {
    /// Reached node.
    pub node: MemoryNode,
    /// Finite ranking score.
    pub score: f32,
    /// Reconstructable path from the seed.
    pub path: ActivationPath,
}

impl ActivationResult {
    /// Creates a result and rejects a non-finite score or a path ending elsewhere.
    ///
    /// # Errors
    ///
    /// Returns a validation error when `score` is not finite or the path does not terminate at
    /// the result node.
    pub fn new(
        node: MemoryNode,
        score: f32,
        path: ActivationPath,
    ) -> Result<Self, ValidationError> {
        if !score.is_finite() {
            return Err(ValidationError::InvalidNumber {
                field: "score",
                min: f32::MIN,
                max: f32::MAX,
            });
        }
        let endpoint = path.steps.last().map_or(path.seed, |step| step.target);
        if endpoint != node.id() {
            return Err(ValidationError::PathEndpointMismatch);
        }
        Ok(Self { node, score, path })
    }

    /// Returns the number of traversed relations.
    #[must_use]
    pub fn hops(&self) -> usize {
        self.path.steps.len()
    }
}

fn validate_string(
    value: String,
    field: &'static str,
    max_bytes: usize,
) -> Result<String, ValidationError> {
    if value.trim().is_empty() {
        return Err(ValidationError::Empty(field));
    }
    if value.len() > max_bytes {
        return Err(ValidationError::TooLong { field, max_bytes });
    }
    Ok(value)
}

pub(crate) fn normalize_terms(value: &str) -> Vec<String> {
    let mut terms = std::collections::BTreeSet::new();
    for token in value.split(|character: char| !character.is_alphanumeric()) {
        if !token.is_empty() {
            terms.insert(token.to_lowercase());
        }
    }
    terms.into_iter().collect()
}

fn validate_unit_interval(value: f32, field: &'static str) -> Result<(), ValidationError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(ValidationError::InvalidNumber {
            field,
            min: 0.0,
            max: 1.0,
        })
    }
}

fn validate_range(
    value: usize,
    field: &'static str,
    min: usize,
    max: usize,
) -> Result<(), ValidationError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(ValidationError::OutOfRange { field, min, max })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u128) -> Result<NodeId, ValidationError> {
        NodeId::new(value)
    }

    #[test]
    fn rejects_zero_node_id() {
        assert_eq!(NodeId::new(0), Err(ValidationError::ZeroId));
    }

    #[test]
    fn rejects_empty_text_and_relation_name() -> Result<(), ValidationError> {
        let kind = MemoryKind::new("decision")?;
        assert!(matches!(
            MemoryNode::new(id(1)?, kind, "  ", 0.5),
            Err(ValidationError::Empty("text"))
        ));
        assert!(matches!(
            Relation::new(id(1)?, id(2)?, "\n", 0.5),
            Err(ValidationError::Empty("relation name"))
        ));
        Ok(())
    }

    #[test]
    fn accepts_weight_boundaries_and_rejects_invalid_weights() -> Result<(), ValidationError> {
        assert!(Relation::new(id(1)?, id(2)?, "owns", 0.0).is_ok());
        assert!(Relation::new(id(1)?, id(2)?, "owns", 1.0).is_ok());
        for weight in [-0.1, 1.1, f32::NAN, f32::INFINITY] {
            assert!(Relation::new(id(1)?, id(2)?, "owns", weight).is_err());
        }
        Ok(())
    }

    #[test]
    fn enforces_each_activation_option_cap() {
        let valid = ActivationOptions::default();
        assert!(valid.validate().is_ok());
        for invalid in [
            ActivationOptions {
                hops: MAX_HOPS + 1,
                ..valid
            },
            ActivationOptions {
                seed_limit: MAX_SEEDS + 1,
                ..valid
            },
            ActivationOptions { limit: 0, ..valid },
            ActivationOptions {
                max_visited_nodes: MAX_VISITED_NODES + 1,
                ..valid
            },
            ActivationOptions {
                max_visited_edges: MAX_VISITED_EDGES + 1,
                ..valid
            },
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn result_path_must_reach_result_node() -> Result<(), ValidationError> {
        let node = MemoryNode::new(id(3)?, MemoryKind::new("incident")?, "session loss", 1.0)?;
        let relation = Relation::new(id(1)?, id(2)?, "caused", 0.9)?;
        let path = ActivationPath {
            seed: id(1)?,
            steps: vec![PathStep::new(&relation)],
        };
        assert!(ActivationResult::new(node, 0.8, path).is_err());
        Ok(())
    }
}
