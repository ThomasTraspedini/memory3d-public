use std::{collections::BTreeMap, error::Error};

use memory3d_core::{
    ActivationOptions, FeedbackKind, MemoryKind, NewFeedbackEvent, NewMemory, NewRelation, NodeId,
    Repository,
};

pub const QUERY: &str = "community routing probe";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertionOrder {
    Forward,
    Reverse,
}

impl InsertionOrder {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Reverse => "reverse",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureKind {
    ParameterSelection,
    NoisyHub,
    CrossModule,
}

impl FixtureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParameterSelection => "community-parameter-selection-v1",
            Self::NoisyHub => "noisy-hub-community-v1",
            Self::CrossModule => "cross-module-project-memory-v1",
        }
    }
}

pub struct Fixture {
    pub relevant: Vec<&'static str>,
}

struct NodeSpec {
    key: &'static str,
    kind: &'static str,
    text: &'static str,
    importance: f32,
}

struct EdgeSpec {
    source: &'static str,
    target: &'static str,
    name: &'static str,
    weight: f32,
}

pub fn ingest(
    repository: &mut Repository,
    kind: FixtureKind,
    order: InsertionOrder,
) -> Result<Fixture, Box<dyn Error>> {
    let (mut nodes, mut edges, relevant) = specs(kind);
    if order == InsertionOrder::Reverse {
        nodes.reverse();
        edges.reverse();
    }
    let mut ids = BTreeMap::<&str, NodeId>::new();
    for node in nodes {
        let stored = repository.add_node(NewMemory::new(
            MemoryKind::new(node.kind)?,
            node.text,
            node.importance,
        )?)?;
        ids.insert(node.key, stored.id());
    }
    for edge in edges {
        repository.add_relation(NewRelation::new(
            ids[edge.source],
            ids[edge.target],
            edge.name,
            edge.weight,
        )?)?;
    }
    if kind != FixtureKind::ParameterSelection {
        let seed = ids["seed"];
        let feedback_target = ids["feedback_target"];
        repository.record_relation_feedback(NewFeedbackEvent::new(
            seed,
            feedback_target,
            "mentions",
            FeedbackKind::Positive,
            1.0,
            1_000,
        )?)?;
    }
    Ok(Fixture { relevant })
}

pub fn options(kind: FixtureKind) -> ActivationOptions {
    match kind {
        FixtureKind::ParameterSelection => ActivationOptions {
            hops: 2,
            seed_limit: 1,
            limit: 5,
            max_visited_nodes: 6,
            max_visited_edges: 5,
            include_seeds: false,
        },
        FixtureKind::NoisyHub => ActivationOptions {
            hops: 2,
            seed_limit: 1,
            limit: 4,
            max_visited_nodes: 5,
            max_visited_edges: 4,
            include_seeds: false,
        },
        FixtureKind::CrossModule => ActivationOptions {
            hops: 1,
            seed_limit: 1,
            limit: 1,
            max_visited_nodes: 2,
            max_visited_edges: 1,
            include_seeds: false,
        },
    }
}

fn specs(kind: FixtureKind) -> (Vec<NodeSpec>, Vec<EdgeSpec>, Vec<&'static str>) {
    match kind {
        FixtureKind::ParameterSelection => parameter_selection_specs(),
        FixtureKind::NoisyHub => noisy_hub_specs(),
        FixtureKind::CrossModule => cross_module_specs(),
    }
}

fn parameter_selection_specs() -> (Vec<NodeSpec>, Vec<EdgeSpec>, Vec<&'static str>) {
    (
        vec![
            node("seed", "component", "Community routing probe selector", 1.0),
            node("a", "evidence", "Selection reciprocal peer A", 0.9),
            node("b", "evidence", "Selection reciprocal peer B", 0.9),
            node("bridge", "dependency", "Selection one-way bridge", 0.8),
            node("small", "note", "Selection disconnected singleton", 0.4),
        ],
        vec![
            edge("seed", "a", "supports", 0.90),
            edge("a", "seed", "supports", 0.90),
            edge("a", "b", "supports", 0.86),
            edge("b", "a", "supports", 0.86),
            edge("b", "bridge", "depends_on", 0.60),
        ],
        vec!["Selection reciprocal peer A", "Selection reciprocal peer B"],
    )
}

fn noisy_hub_specs() -> (Vec<NodeSpec>, Vec<EdgeSpec>, Vec<&'static str>) {
    let mut nodes = vec![
        node(
            "seed",
            "component",
            "Community routing probe auth service",
            1.0,
        ),
        node("auth", "evidence", "Auth community decision", 1.0),
        node("incident", "incident", "Mobile session loss evidence", 1.0),
        node("shared", "dependency", "Shared token dependency", 0.9),
        node("hub", "hub", "High degree release hub", 0.8),
        node(
            "feedback_target",
            "archive",
            "Feedback-selected archive",
            0.2,
        ),
        node("isolated", "note", "Small disconnected control", 0.3),
    ];
    let mut edges = vec![
        edge("seed", "auth", "supports", 0.90),
        edge("auth", "seed", "supports", 0.90),
        edge("auth", "incident", "supports", 0.92),
        edge("incident", "auth", "supports", 0.92),
        edge("incident", "shared", "supports", 0.88),
        edge("shared", "incident", "supports", 0.88),
        edge("seed", "hub", "mentions", 0.99),
        edge("seed", "feedback_target", "mentions", 0.70),
    ];
    for index in 0..8 {
        let key = Box::leak(format!("noise-{index}").into_boxed_str());
        let text = Box::leak(format!("Hub tail noise {index}").into_boxed_str());
        nodes.push(node(key, "archive", text, 0.1));
        edges.push(edge("hub", key, "mentions", 0.99));
    }
    (
        nodes,
        edges,
        vec![
            "Auth community decision",
            "Mobile session loss evidence",
            "Shared token dependency",
        ],
    )
}

fn cross_module_specs() -> (Vec<NodeSpec>, Vec<EdgeSpec>, Vec<&'static str>) {
    (
        vec![
            node(
                "seed",
                "component",
                "Community routing probe token refresh",
                1.0,
            ),
            node("local", "evidence", "Local auth implementation note", 0.7),
            node(
                "mobile",
                "incident",
                "Cross-module mobile session incident",
                1.0,
            ),
            node(
                "checkout",
                "dependency",
                "Checkout authenticated-state dependency",
                1.0,
            ),
            node("shared", "dependency", "Shared identity dependency", 0.9),
            node(
                "feedback_target",
                "archive",
                "Feedback-selected project archive",
                0.2,
            ),
            node("isolated", "note", "Disconnected project note", 0.3),
        ],
        vec![
            edge("seed", "local", "supports", 0.90),
            edge("local", "seed", "supports", 0.90),
            edge("seed", "mobile", "depends_on", 0.99),
            edge("mobile", "checkout", "caused", 0.90),
            edge("checkout", "mobile", "caused", 0.90),
            edge("mobile", "shared", "depends_on", 0.80),
            edge("checkout", "shared", "depends_on", 0.80),
            edge("seed", "feedback_target", "mentions", 0.70),
        ],
        vec![
            "Cross-module mobile session incident",
            "Checkout authenticated-state dependency",
            "Shared identity dependency",
        ],
    )
}

fn node(key: &'static str, kind: &'static str, text: &'static str, importance: f32) -> NodeSpec {
    NodeSpec {
        key,
        kind,
        text,
        importance,
    }
}

fn edge(source: &'static str, target: &'static str, name: &'static str, weight: f32) -> EdgeSpec {
    EdgeSpec {
        source,
        target,
        name,
        weight,
    }
}

pub fn recall<'a>(results: impl Iterator<Item = &'a str>, relevant: &[&str]) -> f64 {
    let found = results
        .filter(|text| relevant.contains(text))
        .fold(0.0, |count, _| count + 1.0);
    let denominator = relevant.iter().fold(0.0, |count, _| count + 1.0);
    found / denominator
}

pub fn paths_valid(
    repository: &Repository,
    results: &[memory3d_core::ActivationResult],
) -> Result<bool, Box<dyn Error>> {
    for result in results {
        let mut current = result.path.seed;
        for step in &result.path.steps {
            if step.source != current
                || repository
                    .get_relation(step.source, step.target, &step.relation)?
                    .is_none()
            {
                return Ok(false);
            }
            current = step.target;
        }
        if current != result.node.id() {
            return Ok(false);
        }
    }
    Ok(true)
}
