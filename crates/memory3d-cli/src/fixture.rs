use std::error::Error;

use memory3d_core::{
    ActivationOptions, EvidenceBundle, EvidenceContextOptions, MemoryKind, MemoryNode, NewMemory,
    NewRelation, NodeId,
};
use serde_json::json;

pub const RECALL_QUERY: &str = "What should I remember before changing token refresh?";
pub const LIFECYCLE_QUERY: &str = "bedroom humidity lifecycle review";
pub const CONFLICT_QUERY: &str = "bedroom ventilation conflict review";
pub const LIFECYCLE_ACTOR: &str = "operator:home-maintenance";
pub const LIFECYCLE_POLICY: &str = "manual-environment-review-v1";
pub const LIFECYCLE_EVIDENCE_IDS: &[&str] = &[
    "bedroom-calibrated-normal-v2",
    "bedroom-high-v1",
    "bedroom-inspect-v1",
    "bedroom-monitor-v2",
    "bedroom-sensor-miscalibrated-v1",
    "bedroom-ventilation-closed-v1",
    "bedroom-ventilation-open-v1",
    "nursery-high-v1",
];

const NODES: &[(&str, &str, f32)] = &[
    (
        "component",
        "Token refresh implementation and rotation flow",
        1.0,
    ),
    (
        "component",
        "AuthService owns authentication lifecycle",
        0.9,
    ),
    (
        "component",
        "MobileApp maintains signed-in user sessions",
        0.9,
    ),
    (
        "incident",
        "Mobile session loss incident after refresh rollout",
        1.0,
    ),
    (
        "component",
        "Checkout requires authenticated user state",
        0.8,
    ),
    (
        "policy",
        "RetryPolicy controls bounded request retries",
        0.7,
    ),
    ("risk", "Network instability disrupts mobile requests", 0.9),
    (
        "decision",
        "Tokens rotate without storing plaintext credentials",
        0.9,
    ),
    (
        "dependency",
        "Session cache stores short-lived authentication state",
        0.8,
    ),
    (
        "operation",
        "Alert when refresh failure rate exceeds threshold",
        0.8,
    ),
    (
        "test",
        "Mobile refresh regression suite covers expired tokens",
        0.8,
    ),
    (
        "risk",
        "Clock skew can invalidate otherwise current tokens",
        0.7,
    ),
    ("component", "ApiGateway validates access tokens", 0.8),
    ("component", "UserProfile serves customer preferences", 0.5),
    ("component", "Inventory tracks warehouse stock", 0.5),
    ("component", "TaxCalculator computes regional tax", 0.5),
    (
        "incident",
        "Search indexing lag delayed product discovery",
        0.4,
    ),
    (
        "decision",
        "Audit logs retain security events for ninety days",
        0.6,
    ),
    ("operation", "Nightly backups verify restore checksums", 0.6),
    ("component", "EmailWorker sends transactional messages", 0.4),
    ("risk", "Payment provider latency may delay checkout", 0.5),
    ("component", "RecommendationEngine ranks catalog items", 0.4),
    ("preference", "Prefer reversible database migrations", 0.7),
    (
        "decision",
        "Feature flags guard risky production rollouts",
        0.7,
    ),
    (
        "component",
        "ImagePipeline produces catalog thumbnails",
        0.3,
    ),
    (
        "operation",
        "Support dashboard summarizes open incidents",
        0.4,
    ),
    ("test", "Contract tests pin payment provider responses", 0.5),
    ("risk", "Queue saturation can postpone email delivery", 0.4),
];

const RELATIONS: &[(usize, usize, &str, f32)] = &[
    (1, 0, "owns", 1.0),
    (2, 0, "depends_on", 0.95),
    (0, 3, "caused", 1.0),
    (4, 2, "depends_on", 0.9),
    (5, 6, "mitigates", 0.9),
    (0, 8, "uses", 0.85),
    (0, 9, "monitored_by", 0.8),
    (0, 10, "verified_by", 0.9),
    (0, 11, "threatened_by", 0.75),
    (12, 0, "validates", 0.8),
    (3, 10, "prevented_by", 0.85),
    (2, 6, "affected_by", 0.7),
    (6, 5, "handled_by", 0.9),
    (4, 20, "threatened_by", 0.6),
    (20, 26, "covered_by", 0.8),
    (19, 27, "affected_by", 0.7),
    (23, 3, "guards_rollout_of", 0.65),
    (17, 9, "feeds", 0.55),
    (3, 25, "documented_in", 0.8),
    (8, 7, "constrained_by", 0.75),
];

pub fn nodes() -> Result<Vec<NewMemory>, Box<dyn Error>> {
    NODES
        .iter()
        .map(|(kind, text, importance)| {
            Ok(NewMemory::new(MemoryKind::new(*kind)?, *text, *importance)?)
        })
        .collect()
}

pub fn relations(nodes: &[MemoryNode]) -> Result<Vec<NewRelation>, Box<dyn Error>> {
    RELATIONS
        .iter()
        .map(|(source, target, name, weight)| {
            Ok(NewRelation::new(
                nodes[*source].id(),
                nodes[*target].id(),
                *name,
                *weight,
            )?)
        })
        .collect()
}

pub fn recall_options() -> ActivationOptions {
    ActivationOptions {
        hops: 3,
        seed_limit: 1,
        limit: 10,
        max_visited_nodes: 64,
        max_visited_edges: 128,
        include_seeds: false,
    }
}

pub fn lifecycle_nodes() -> Result<Vec<NewMemory>, Box<dyn Error>> {
    [
        ("room", LIFECYCLE_QUERY, 1.0),
        ("room", CONFLICT_QUERY, 1.0),
        (
            "environment",
            "Room environmental memory connects sensor history with maintenance guidance",
            0.9,
        ),
    ]
    .into_iter()
    .map(|(kind, text, importance)| Ok(NewMemory::new(MemoryKind::new(kind)?, text, importance)?))
    .collect()
}

#[allow(clippy::too_many_lines)]
pub fn lifecycle_bundles() -> Result<Vec<EvidenceBundle>, Box<dyn Error>> {
    let bedroom_scope = json!({"home":"demo-home","room":"bedroom"});
    let nursery_scope = json!({"home":"demo-home","room":"nursery"});
    let decision = |supporting_evidence_ids: &[&str], decided_at_ms| {
        json!({
            "actor":LIFECYCLE_ACTOR,
            "policy":LIFECYCLE_POLICY,
            "supporting_evidence_ids":supporting_evidence_ids,
            "decided_at_ms":decided_at_ms
        })
    };
    let initial = json!({
        "version":1,
        "idempotency_key":"lifecycle-initial-v1",
        "items":[
            {
                "id":"bedroom-high-v1",
                "kind":"observation",
                "text":"Sensor H-17 reported persistently high humidity.",
                "source":"sensor:H-17",
                "producer":"import:manual-demo",
                "scope":bedroom_scope,
                "lifecycle":"approved",
                "created_at_ms":1_700_000_000_000_i64,
                "observed_at_ms":1_700_000_000_000_i64,
                "decision":decision(&[], 1_700_000_000_100_i64)
            },
            {
                "id":"bedroom-inspect-v1",
                "kind":"recommendation",
                "text":"Inspect the bedroom for a persistent moisture source.",
                "source":"procedure:humidity-response",
                "producer":"operator:home-maintenance",
                "scope":bedroom_scope,
                "lifecycle":"approved",
                "created_at_ms":1_700_000_001_000_i64,
                "decision":decision(&["bedroom-high-v1"], 1_700_000_001_100_i64)
            },
            {
                "id":"nursery-high-v1",
                "kind":"observation",
                "text":"A calibrated sensor reported high humidity in the nursery.",
                "source":"sensor:N-04",
                "producer":"import:manual-demo",
                "scope":nursery_scope,
                "lifecycle":"approved",
                "created_at_ms":1_700_000_001_500_i64,
                "observed_at_ms":1_700_000_001_500_i64,
                "decision":decision(&[], 1_700_000_001_600_i64)
            }
        ]
    });
    let corrective = json!({
        "version":1,
        "idempotency_key":"lifecycle-correction-v2",
        "items":[
            {
                "id":"bedroom-calibrated-normal-v2",
                "kind":"observation",
                "text":"Calibrated instrument C-02 measured normal humidity.",
                "source":"instrument:C-02",
                "producer":"operator:home-maintenance",
                "scope":bedroom_scope,
                "lifecycle":"observed",
                "created_at_ms":1_700_000_002_000_i64,
                "observed_at_ms":1_700_000_002_000_i64,
                "supersedes":["bedroom-high-v1"],
                "decision":decision(&["bedroom-high-v1"], 1_700_000_002_100_i64)
            },
            {
                "id":"bedroom-monitor-v2",
                "kind":"recommendation",
                "text":"Continue routine monitoring; no moisture inspection is currently indicated.",
                "source":"procedure:humidity-response",
                "producer":"operator:home-maintenance",
                "scope":bedroom_scope,
                "lifecycle":"approved",
                "created_at_ms":1_700_000_003_000_i64,
                "supersedes":["bedroom-inspect-v1"],
                "decision":decision(&["bedroom-calibrated-normal-v2"], 1_700_000_003_100_i64)
            },
            {
                "id":"bedroom-sensor-miscalibrated-v1",
                "kind":"finding",
                "text":"Inspection found sensor H-17 was miscalibrated.",
                "source":"inspection:calibration-42",
                "producer":"operator:home-maintenance",
                "scope":bedroom_scope,
                "lifecycle":"approved",
                "created_at_ms":1_700_000_002_500_i64,
                "derives_from":["bedroom-high-v1"],
                "decision":decision(&["bedroom-high-v1"], 1_700_000_002_600_i64)
            }
        ]
    });
    let conflict = json!({
        "version":1,
        "idempotency_key":"lifecycle-conflict-v1",
        "items":[
            {
                "id":"bedroom-ventilation-closed-v1",
                "kind":"observation",
                "text":"Inspection note says the bedroom vent was closed.",
                "source":"inspection:ventilation-a",
                "producer":"operator:home-maintenance",
                "scope":bedroom_scope,
                "lifecycle":"observed",
                "created_at_ms":1_700_000_004_000_i64,
                "observed_at_ms":1_700_000_004_000_i64,
                "conflict_group":"bedroom-ventilation-state",
                "conflicts_with":["bedroom-ventilation-open-v1"]
            },
            {
                "id":"bedroom-ventilation-open-v1",
                "kind":"observation",
                "text":"Inspection note says the bedroom vent was open.",
                "source":"inspection:ventilation-b",
                "producer":"operator:home-maintenance",
                "scope":bedroom_scope,
                "lifecycle":"observed",
                "created_at_ms":1_700_000_004_100_i64,
                "observed_at_ms":1_700_000_004_100_i64,
                "conflict_group":"bedroom-ventilation-state"
            }
        ]
    });
    [initial, corrective, conflict]
        .into_iter()
        .map(|bundle| Ok(EvidenceBundle::from_json(&bundle.to_string())?))
        .collect()
}

pub fn lifecycle_relations(
    nodes: &[MemoryNode],
    evidence_nodes: &std::collections::BTreeMap<String, NodeId>,
) -> Result<Vec<NewRelation>, Box<dyn Error>> {
    let evidence = |id: &str| -> Result<NodeId, Box<dyn Error>> {
        evidence_nodes
            .get(id)
            .copied()
            .ok_or_else(|| format!("missing lifecycle evidence mapping for {id}").into())
    };
    let mut relations = vec![
        NewRelation::new(nodes[0].id(), nodes[2].id(), "connects_to", 0.94)?,
        NewRelation::new(nodes[1].id(), nodes[2].id(), "connects_to", 0.94)?,
    ];
    for (id, weight) in [
        ("bedroom-calibrated-normal-v2", 1.0),
        ("bedroom-monitor-v2", 0.99),
        ("bedroom-sensor-miscalibrated-v1", 0.98),
        ("bedroom-inspect-v1", 0.97),
        ("bedroom-high-v1", 0.96),
        ("nursery-high-v1", 0.95),
    ] {
        relations.push(NewRelation::new(
            nodes[0].id(),
            evidence(id)?,
            "recalls",
            weight,
        )?);
    }
    for (id, weight) in [
        ("bedroom-ventilation-closed-v1", 1.0),
        ("bedroom-ventilation-open-v1", 0.99),
    ] {
        relations.push(NewRelation::new(
            nodes[1].id(),
            evidence(id)?,
            "recalls",
            weight,
        )?);
    }
    Ok(relations)
}

pub fn lifecycle_activation_options() -> ActivationOptions {
    ActivationOptions {
        hops: 1,
        seed_limit: 1,
        limit: 10,
        max_visited_nodes: 16,
        max_visited_edges: 16,
        include_seeds: false,
    }
}

pub fn lifecycle_context_options(
    room: &str,
    candidate_scan_limit: usize,
) -> Result<EvidenceContextOptions, Box<dyn Error>> {
    let scope = std::collections::BTreeMap::from([
        ("home".to_owned(), "demo-home".to_owned()),
        ("room".to_owned(), room.to_owned()),
    ]);
    Ok(EvidenceContextOptions::settled(scope)?
        .with_activation(lifecycle_activation_options())
        .with_candidate_scan_limit(candidate_scan_limit))
}
