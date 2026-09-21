use std::error::Error;

use memory3d_core::{ActivationOptions, MemoryKind, MemoryNode, NewMemory, NewRelation};

pub const RECALL_QUERY: &str = "What should I remember before changing token refresh?";

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
