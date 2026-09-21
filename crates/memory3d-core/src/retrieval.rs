use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::{Duration, Instant},
};

use rusqlite::{ToSql, params_from_iter};

use crate::{
    ActivationOptions, ActivationPath, ActivationResult, MAX_QUERY_BYTES, MAX_QUERY_TERMS,
    MAX_RESULTS, MemoryKind, MemoryNode, NodeId, PathStep, Relation, Repository, RepositoryError,
    ValidationError, normalize_terms, validate_range,
};

const ADJACENCY_SQL: &str = "SELECT r.source_id, r.target_id, r.name, r.weight, r.created_at_ms, r.updated_at_ms, r.reinforcement_count FROM relations r INDEXED BY relations_adjacency_idx JOIN nodes n ON n.id = r.target_id WHERE r.source_id = ?1 ORDER BY r.weight DESC, n.importance DESC, n.kind ASC, n.text ASC, r.target_id ASC, r.name ASC LIMIT ?2";

/// Options for deterministic lexical retrieval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    /// Maximum number of results.
    pub limit: usize,
    /// Optional exact memory-kind filter.
    pub kind: Option<MemoryKind>,
}

impl SearchOptions {
    /// Validates the result limit against the core hard cap.
    ///
    /// # Errors
    ///
    /// Returns a validation error when `limit` is zero or exceeds [`MAX_RESULTS`].
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_range(self.limit, "limit", 1, MAX_RESULTS)
    }
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 10,
            kind: None,
        }
    }
}

/// A lexical match ranked by normalized query-term coverage.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    /// Matched stored node.
    pub node: MemoryNode,
    /// Number of distinct query terms found in the node's kind or text.
    pub matched_terms: usize,
    /// `matched_terms / distinct_query_terms`, in `(0, 1]`.
    pub score: f32,
}

/// Work consumed by one bounded activation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TraversalStats {
    /// Lexical seeds selected.
    pub seeds: usize,
    /// Distinct seed or reached nodes admitted under the node budget.
    pub visited_nodes: usize,
    /// Stored outgoing relations examined under the edge budget.
    pub visited_edges: usize,
}

/// Database calls issued by one activation, separated by retrieval phase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DatabaseStats {
    /// Batched lexical-index queries used to choose seeds.
    pub seed_queries: usize,
    /// Node records loaded for lexical seeds.
    pub seed_node_reads: usize,
    /// Bounded outgoing-relation queries issued during graph traversal.
    pub adjacency_queries: usize,
    /// Non-seed node records loaded during graph traversal.
    pub traversal_node_reads: usize,
}

/// Read-only evidence about one bounded adjacency query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdjacencyInspection {
    /// All stored outgoing rows that are candidates before `LIMIT`.
    pub candidate_rows: usize,
    /// Rows admitted by the bounded query.
    pub returned_rows: usize,
    /// `SQLite` `EXPLAIN QUERY PLAN` detail rows in execution order.
    pub query_plan: Vec<String>,
    /// Time spent executing and decoding only the bounded adjacency query.
    pub query_time: Duration,
}

impl AdjacencyInspection {
    /// Reports whether `SQLite` selected the dedicated evidence-first adjacency index.
    #[must_use]
    pub fn uses_adjacency_index(&self) -> bool {
        self.query_plan
            .iter()
            .any(|detail| detail.contains("relations_adjacency_idx"))
    }

    /// Reports whether `SQLite` retained a temporary sort for evidence tie-break columns.
    #[must_use]
    pub fn uses_temporary_sort(&self) -> bool {
        self.query_plan
            .iter()
            .any(|detail| detail.contains("TEMP B-TREE"))
    }
}

/// Wall-clock phase timings captured for diagnostics and benchmark reports.
///
/// These values are observational and must not be used to alter ranking or traversal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActivationTimings {
    /// Time spent validating the query and selecting lexical seeds.
    pub seed_lookup: Duration,
    /// Time spent in bounded graph traversal and result ranking.
    pub graph: Duration,
}

/// Ranked activation results and auditable traversal counters.
#[derive(Debug, Clone)]
pub struct ActivationReport {
    /// Results ordered by descending score, then stable node ID.
    pub results: Vec<ActivationResult>,
    /// Work consumed while producing the results.
    pub stats: TraversalStats,
    /// Database operations issued while selecting seeds and traversing the graph.
    pub database: DatabaseStats,
    /// Source node for each adjacency query, in execution order.
    ///
    /// This trace lets benchmarks audit storage candidate work separately from admitted edges.
    pub adjacency_sources: Vec<NodeId>,
    /// Observed time split between seed lookup and graph work.
    pub timings: ActivationTimings,
}

#[derive(Clone)]
struct State {
    node: NodeId,
    seed: NodeId,
    propagation: f32,
    steps: Vec<PathStep>,
}

struct Candidate {
    node: MemoryNode,
    score: f32,
    seed: NodeId,
    steps: Vec<PathStep>,
}

impl Repository {
    /// Finds nodes containing exact normalized query terms in their kind or text.
    ///
    /// Normalization splits on non-alphanumeric characters, applies Unicode lowercase, and
    /// deduplicates terms. Results rank by matched query-term count, node importance, kind, text,
    /// and finally ID.
    ///
    /// # Errors
    ///
    /// Returns a validation error for a blank, oversized, or over-tokenized query or invalid
    /// options, and a repository error for malformed stored data or database failure.
    pub fn search(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<SearchResult>, RepositoryError> {
        options.validate()?;
        let terms = validate_query(query)?;
        self.search_terms(&terms, options.limit, options.kind.as_ref())
    }

    /// Runs deterministic bounded activation over outgoing stored relations.
    ///
    /// For a path of `h` edges, the score is `seed_score * product(edge_weight) * 0.8^h *
    /// (0.5 + 0.5 * node_importance)`. The best path per node wins; exact ties use the lower seed
    /// ID and then the lexicographically smaller stored relation path.
    ///
    /// # Errors
    ///
    /// Returns a validation error for the query or options, or a repository error while reading
    /// stored nodes and relations.
    pub fn activate(
        &self,
        query: &str,
        options: &ActivationOptions,
    ) -> Result<ActivationReport, RepositoryError> {
        let seed_started = Instant::now();
        options.validate()?;
        let terms = validate_query(query)?;
        let seed_limit = options.seed_limit.min(options.max_visited_nodes);
        let seeds = self.search_terms(&terms, seed_limit, None)?;
        let seed_lookup = seed_started.elapsed();
        let mut stats = TraversalStats {
            seeds: seeds.len(),
            visited_nodes: seeds.len(),
            visited_edges: 0,
        };
        let mut database = DatabaseStats {
            seed_queries: 1,
            seed_node_reads: seeds.len(),
            ..DatabaseStats::default()
        };
        if seeds.is_empty() {
            return Ok(ActivationReport {
                results: Vec::new(),
                stats,
                database,
                adjacency_sources: Vec::new(),
                timings: ActivationTimings {
                    seed_lookup,
                    graph: Duration::ZERO,
                },
            });
        }

        let graph_started = Instant::now();
        let mut adjacency_sources = Vec::new();
        let candidates = self.traverse_graph(
            seeds,
            options,
            &mut stats,
            &mut database,
            &mut adjacency_sources,
        )?;
        finish_activation(
            candidates,
            stats,
            database,
            adjacency_sources,
            ActivationTimings {
                seed_lookup,
                graph: graph_started.elapsed(),
            },
            options.limit,
        )
    }

    /// Inspects candidate work, query plan, and execution time for one adjacency batch.
    ///
    /// `candidate_rows` is an exact count of stored outgoing rows. It is a conservative upper
    /// bound on rows `SQLite` may need to inspect/order because the planner can stop early after
    /// `LIMIT` when the leading weight order is selective.
    ///
    /// # Errors
    ///
    /// Returns a missing-node, validation, decoding, or database error.
    pub fn inspect_adjacency(
        &self,
        source: NodeId,
        limit: usize,
    ) -> Result<AdjacencyInspection, RepositoryError> {
        validate_range(limit, "adjacency limit", 1, crate::MAX_VISITED_EDGES)?;
        if self.get_node(source)?.is_none() {
            return Err(RepositoryError::NodeNotFound(source));
        }
        let source = super::repository::id_to_i64(source)?;
        let limit = i64::try_from(limit).map_err(|_| {
            RepositoryError::InvalidDatabase("edge limit exceeds SQLite range".to_owned())
        })?;
        let candidate_rows = self.connection.query_row(
            "SELECT count(*) FROM relations WHERE source_id = ?1",
            [source],
            |row| row.get::<_, i64>(0),
        )?;
        let candidate_rows = usize::try_from(candidate_rows).map_err(|_| {
            RepositoryError::InvalidDatabase("adjacency count exceeds memory range".to_owned())
        })?;
        let query_plan = {
            let sql = format!("EXPLAIN QUERY PLAN {ADJACENCY_SQL}");
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params![source, limit], |row| row.get(3))?;
            rows.collect::<Result<Vec<String>, _>>()?
        };
        let started = Instant::now();
        let returned_rows = self
            .outgoing_relations(
                super::repository::id_from_i64(source)?,
                usize::try_from(limit).map_err(|_| {
                    RepositoryError::InvalidDatabase("edge limit exceeds memory range".to_owned())
                })?,
            )?
            .len();
        Ok(AdjacencyInspection {
            candidate_rows,
            returned_rows,
            query_plan,
            query_time: started.elapsed(),
        })
    }

    fn traverse_graph(
        &self,
        seeds: Vec<SearchResult>,
        options: &ActivationOptions,
        stats: &mut TraversalStats,
        database: &mut DatabaseStats,
        adjacency_sources: &mut Vec<NodeId>,
    ) -> Result<BTreeMap<NodeId, Candidate>, RepositoryError> {
        let seed_ids: BTreeSet<_> = seeds.iter().map(|seed| seed.node.id()).collect();
        let mut visited = seed_ids.clone();
        let mut nodes = BTreeMap::new();
        let mut queue = VecDeque::new();
        let mut candidates = BTreeMap::new();

        for seed in seeds {
            let id = seed.node.id();
            nodes.insert(id, seed.node.clone());
            queue.push_back(State {
                node: id,
                seed: id,
                propagation: seed.score,
                steps: Vec::new(),
            });
            if options.include_seeds {
                consider_candidate(
                    &mut candidates,
                    seed.node,
                    seed.score * importance_factor(nodes[&id].importance()),
                    id,
                    Vec::new(),
                );
            }
        }

        let mut best_state: BTreeMap<(NodeId, usize), (f32, NodeId, Vec<PathStep>)> =
            BTreeMap::new();
        while let Some(state) = queue.pop_front() {
            let hop = state.steps.len();
            if hop >= usize::from(options.hops)
                || stats.visited_edges >= options.max_visited_edges
                || stats.visited_nodes >= options.max_visited_nodes
            {
                continue;
            }
            let remaining_edges = options.max_visited_edges - stats.visited_edges;
            database.adjacency_queries += 1;
            adjacency_sources.push(state.node);
            let relations = self.outgoing_relations(state.node, remaining_edges)?;
            for relation in relations {
                stats.visited_edges += 1;
                let target = relation.target();
                if path_contains(&state, target) {
                    continue;
                }
                if !visited.contains(&target) {
                    if stats.visited_nodes == options.max_visited_nodes {
                        break;
                    }
                    visited.insert(target);
                    stats.visited_nodes += 1;
                }
                let node = if let Some(node) = nodes.get(&target) {
                    node.clone()
                } else {
                    database.traversal_node_reads += 1;
                    let node = self.get_node(target)?.ok_or_else(|| {
                        RepositoryError::InvalidDatabase(format!(
                            "relation target {target} is missing"
                        ))
                    })?;
                    nodes.insert(target, node.clone());
                    node
                };
                let mut steps = state.steps.clone();
                steps.push(PathStep::new(&relation));
                let propagation = state.propagation * relation.weight() * 0.8;
                let score = propagation * importance_factor(node.importance());
                if options.include_seeds || !seed_ids.contains(&target) {
                    consider_candidate(&mut candidates, node, score, state.seed, steps.clone());
                }
                let key = (target, steps.len());
                if should_replace_state(best_state.get(&key), propagation, state.seed, &steps) {
                    best_state.insert(key, (propagation, state.seed, steps.clone()));
                    queue.push_back(State {
                        node: target,
                        seed: state.seed,
                        propagation,
                        steps,
                    });
                }
                if stats.visited_edges == options.max_visited_edges {
                    break;
                }
            }
        }
        Ok(candidates)
    }

    fn search_terms(
        &self,
        terms: &[String],
        limit: usize,
        kind: Option<&MemoryKind>,
    ) -> Result<Vec<SearchResult>, RepositoryError> {
        let placeholders = std::iter::repeat_n("?", terms.len())
            .collect::<Vec<_>>()
            .join(", ");
        let kind_clause = if kind.is_some() {
            " AND n.kind = ?"
        } else {
            ""
        };
        let sql = format!(
            "SELECT nt.node_id, count(*) AS matches FROM node_terms nt JOIN nodes n ON n.id = nt.node_id WHERE nt.term IN ({placeholders}){kind_clause} GROUP BY nt.node_id ORDER BY matches DESC, n.importance DESC, n.kind ASC, n.text ASC, nt.node_id ASC LIMIT ?"
        );
        let mut values: Vec<&dyn ToSql> = terms.iter().map(|term| term as &dyn ToSql).collect();
        let kind_value = kind.map(MemoryKind::as_str);
        if let Some(kind_value) = &kind_value {
            values.push(kind_value);
        }
        let limit_i64 = i64::try_from(limit).map_err(|_| {
            RepositoryError::InvalidDatabase("search limit exceeds SQLite range".to_owned())
        })?;
        values.push(&limit_i64);
        let matches = {
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let term_count = u16::try_from(terms.len()).map_err(|_| {
            RepositoryError::InvalidDatabase("query term count exceeds scoring range".to_owned())
        })?;
        let denominator = f32::from(term_count);
        matches
            .into_iter()
            .map(|(raw_id, raw_matches)| {
                let id = super::repository::id_from_i64(raw_id)?;
                let node = self.get_node(id)?.ok_or_else(|| {
                    RepositoryError::InvalidDatabase(format!("indexed node {id} is missing"))
                })?;
                let matched_terms = usize::try_from(raw_matches).map_err(|_| {
                    RepositoryError::InvalidDatabase("negative lexical match count".to_owned())
                })?;
                let matched_count = u16::try_from(matched_terms).map_err(|_| {
                    RepositoryError::InvalidDatabase(
                        "lexical match count exceeds scoring range".to_owned(),
                    )
                })?;
                Ok(SearchResult {
                    node,
                    matched_terms,
                    score: f32::from(matched_count) / denominator,
                })
            })
            .collect()
    }

    fn outgoing_relations(
        &self,
        source: NodeId,
        limit: usize,
    ) -> Result<Vec<Relation>, RepositoryError> {
        let limit = i64::try_from(limit).map_err(|_| {
            RepositoryError::InvalidDatabase("edge limit exceeds SQLite range".to_owned())
        })?;
        let mut statement = self.connection.prepare(ADJACENCY_SQL)?;
        let rows = statement.query_map(
            rusqlite::params![super::repository::id_to_i64(source)?, limit],
            super::repository::read_relation_row,
        )?;
        rows.map(|row| super::repository::decode_relation(row?))
            .collect()
    }
}

fn validate_query(query: &str) -> Result<Vec<String>, ValidationError> {
    if query.trim().is_empty() {
        return Err(ValidationError::Empty("query"));
    }
    if query.len() > MAX_QUERY_BYTES {
        return Err(ValidationError::TooLong {
            field: "query",
            max_bytes: MAX_QUERY_BYTES,
        });
    }
    let terms = normalize_terms(query);
    if terms.is_empty() {
        return Err(ValidationError::Empty("query terms"));
    }
    validate_range(terms.len(), "query_terms", 1, MAX_QUERY_TERMS)?;
    Ok(terms)
}

fn importance_factor(importance: f32) -> f32 {
    0.5 + 0.5 * importance
}

fn path_contains(state: &State, target: NodeId) -> bool {
    state.seed == target || state.steps.iter().any(|step| step.target == target)
}

fn path_key(seed: NodeId, steps: &[PathStep]) -> (NodeId, Vec<(NodeId, NodeId, &str)>) {
    (
        seed,
        steps
            .iter()
            .map(|step| (step.source, step.target, step.relation.as_str()))
            .collect(),
    )
}

fn should_replace_state(
    current: Option<&(f32, NodeId, Vec<PathStep>)>,
    score: f32,
    seed: NodeId,
    steps: &[PathStep],
) -> bool {
    current.is_none_or(|(current_score, current_seed, current_steps)| {
        score > *current_score
            || (score.total_cmp(current_score) == std::cmp::Ordering::Equal
                && path_key(seed, steps) < path_key(*current_seed, current_steps))
    })
}

fn consider_candidate(
    candidates: &mut BTreeMap<NodeId, Candidate>,
    node: MemoryNode,
    score: f32,
    seed: NodeId,
    steps: Vec<PathStep>,
) {
    let replace = candidates.get(&node.id()).is_none_or(|current| {
        score > current.score
            || (score.total_cmp(&current.score) == std::cmp::Ordering::Equal
                && path_key(seed, &steps) < path_key(current.seed, &current.steps))
    });
    if replace {
        candidates.insert(
            node.id(),
            Candidate {
                node,
                score,
                seed,
                steps,
            },
        );
    }
}

fn finish_activation(
    candidates: BTreeMap<NodeId, Candidate>,
    stats: TraversalStats,
    database: DatabaseStats,
    adjacency_sources: Vec<NodeId>,
    timings: ActivationTimings,
    limit: usize,
) -> Result<ActivationReport, RepositoryError> {
    let mut ranked = candidates.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| right.node.importance().total_cmp(&left.node.importance()))
            .then_with(|| left.node.kind().as_str().cmp(right.node.kind().as_str()))
            .then_with(|| left.node.text().cmp(right.node.text()))
            .then_with(|| left.node.id().cmp(&right.node.id()))
    });
    ranked.truncate(limit);
    let results = ranked
        .into_iter()
        .map(|candidate| {
            ActivationResult::new(
                candidate.node,
                candidate.score,
                ActivationPath {
                    seed: candidate.seed,
                    steps: candidate.steps,
                },
            )
            .map_err(RepositoryError::from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ActivationReport {
        results,
        stats,
        database,
        adjacency_sources,
        timings,
    })
}
