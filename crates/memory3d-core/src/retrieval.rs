use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::{Duration, Instant},
};

use rusqlite::{ToSql, params_from_iter};

use crate::{
    ActivationOptions, ActivationPath, ActivationResult, CommunityMembership, CommunityPolicy,
    CommunityResolutionSummary, MAX_QUERY_BYTES, MAX_QUERY_TERMS, MAX_RESULTS, MAX_VISITED_EDGES,
    MemoryKind, MemoryNode, NodeId, PathStep, Relation, Repository, RepositoryError,
    ValidationError, community::CommunityRoutingIndex, normalize_terms, validate_range,
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

/// Opt-in explicit feedback policy for one activation call.
///
/// Feedback never mutates raw relation weights. It changes only the effective relation weight used
/// during this call, from explicit events recorded on that same relation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeedbackPolicy {
    /// Positive multiplier added per fresh unit of positive feedback.
    pub positive_boost: f32,
    /// Negative multiplier subtracted per fresh unit of negative feedback.
    pub negative_penalty: f32,
    /// Lower clamp for the relation feedback factor.
    pub min_factor: f32,
    /// Upper clamp for the relation feedback factor.
    pub max_factor: f32,
    /// Optional half-life in milliseconds for deterministic event staleness.
    pub decay_half_life_ms: Option<u64>,
    /// Deterministic clock value used only when decay is enabled.
    pub now_ms: i64,
    /// Maximum outgoing candidates inspected per expanded source before feedback reordering.
    pub candidate_scan_limit: usize,
}

impl FeedbackPolicy {
    /// Task 13's deterministic explicit-feedback policy.
    #[must_use]
    pub const fn explicit_v1() -> Self {
        Self {
            positive_boost: 0.30,
            negative_penalty: 0.60,
            min_factor: 0.05,
            max_factor: 2.0,
            decay_half_life_ms: None,
            now_ms: 0,
            candidate_scan_limit: MAX_VISITED_EDGES,
        }
    }

    /// Returns the same policy with deterministic age decay enabled.
    #[must_use]
    pub const fn with_decay(mut self, now_ms: i64, half_life_ms: u64) -> Self {
        self.now_ms = now_ms;
        self.decay_half_life_ms = Some(half_life_ms);
        self
    }

    /// Validates every feedback-policy bound.
    ///
    /// # Errors
    ///
    /// Returns a validation error for non-finite factors, inverted clamps, invalid timestamps, or
    /// an out-of-range candidate scan limit.
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_range(
            self.candidate_scan_limit,
            "feedback_candidate_scan_limit",
            1,
            MAX_VISITED_EDGES,
        )?;
        for (field, value) in [
            ("positive_boost", self.positive_boost),
            ("negative_penalty", self.negative_penalty),
            ("min_factor", self.min_factor),
            ("max_factor", self.max_factor),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(ValidationError::InvalidNumber {
                    field,
                    min: 0.0,
                    max: f32::MAX,
                });
            }
        }
        if self.min_factor > self.max_factor {
            return Err(ValidationError::InvalidNumber {
                field: "feedback factor clamp",
                min: 0.0,
                max: self.max_factor,
            });
        }
        if self.now_ms < 0 {
            return Err(ValidationError::OutOfRange {
                field: "feedback_now_ms",
                min: 0,
                max: usize::MAX,
            });
        }
        if matches!(self.decay_half_life_ms, Some(0)) {
            return Err(ValidationError::OutOfRange {
                field: "feedback_decay_half_life_ms",
                min: 1,
                max: usize::MAX,
            });
        }
        Ok(())
    }
}

impl Default for FeedbackPolicy {
    fn default() -> Self {
        Self::explicit_v1()
    }
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
    /// Results ordered by descending score, importance, kind, text, then stable node ID.
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
    /// Experimental community-routing diagnostics, present only for community activation calls.
    pub community: Option<CommunityDiagnostics>,
    /// Complete deterministic candidate ranking before the public result limit is applied.
    ranked_candidates: Vec<ActivationResult>,
}

impl ActivationReport {
    pub(crate) fn ranked_candidates(&self) -> &[ActivationResult] {
        &self.ranked_candidates
    }
}

/// How a raw relation candidate was routed by the community overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommunityRoute {
    /// Source and target share a fine community.
    Fine,
    /// Source and target differ at fine resolution but share a coarse community.
    Coarse,
    /// Source and target cross both persisted community resolutions.
    Boundary,
}

impl CommunityRoute {
    /// Stable report label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fine => "fine",
            Self::Coarse => "coarse",
            Self::Boundary => "boundary",
        }
    }
}

/// Community-routing decision for one raw outgoing relation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommunityEdgeDiagnostic {
    /// Raw source node.
    pub source: NodeId,
    /// Raw target node.
    pub target: NodeId,
    /// Stored relation name.
    pub relation: String,
    /// Rank before community routing, using ADR 0005 evidence order.
    pub raw_rank: usize,
    /// Rank after deterministic community routing.
    pub routed_rank: usize,
    /// Resolution relationship used to route the candidate.
    pub route: CommunityRoute,
    /// Persisted memberships proving the source routing input.
    pub source_memberships: Vec<CommunityMembership>,
    /// Persisted memberships proving the target routing input.
    pub target_memberships: Vec<CommunityMembership>,
}

/// Raw-path-to-community provenance for one returned result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommunityPathProvenance {
    /// Returned raw result node.
    pub result: NodeId,
    /// Raw seed followed by every raw path target.
    pub raw_path_nodes: Vec<NodeId>,
    /// Fine and coarse memberships for every raw path node.
    pub memberships: Vec<CommunityMembership>,
}

/// Auditable diagnostics for one community-routed activation.
#[derive(Debug, Clone, PartialEq)]
pub struct CommunityDiagnostics {
    /// Stable policy name.
    pub policy: &'static str,
    /// Frozen parameters used for routing.
    pub parameters: CommunityPolicy,
    /// Raw graph revision shared by build and activation.
    pub graph_revision: u64,
    /// Persisted resolution and membership counts.
    pub resolutions: Vec<CommunityResolutionSummary>,
    /// Raw outgoing candidates inspected before routing.
    pub candidate_edges: usize,
    /// Candidates routed within a fine community.
    pub fine_routed_edges: usize,
    /// Candidates routed only within a coarse community.
    pub coarse_routed_edges: usize,
    /// Candidates crossing both resolutions.
    pub boundary_routed_edges: usize,
    /// Per-candidate raw provenance and routing decisions.
    pub edges: Vec<CommunityEdgeDiagnostic>,
    /// Per-result membership provenance along the reconstructable raw path.
    pub results: Vec<CommunityPathProvenance>,
}

impl CommunityDiagnostics {
    fn new(policy: CommunityPolicy, index: &CommunityRoutingIndex) -> Self {
        Self {
            policy: "multi-resolution-v1",
            parameters: policy,
            graph_revision: index.revision,
            resolutions: index.summaries.clone(),
            candidate_edges: 0,
            fine_routed_edges: 0,
            coarse_routed_edges: 0,
            boundary_routed_edges: 0,
            edges: Vec::new(),
            results: Vec::new(),
        }
    }

    fn record_edge(&mut self, diagnostic: CommunityEdgeDiagnostic) {
        self.candidate_edges += 1;
        match diagnostic.route {
            CommunityRoute::Fine => self.fine_routed_edges += 1,
            CommunityRoute::Coarse => self.coarse_routed_edges += 1,
            CommunityRoute::Boundary => self.boundary_routed_edges += 1,
        }
        self.edges.push(diagnostic);
    }
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

struct TraversalContext<'a> {
    stats: &'a mut TraversalStats,
    database: &'a mut DatabaseStats,
    adjacency_sources: &'a mut Vec<NodeId>,
    feedback_policy: Option<FeedbackPolicy>,
    community_policy: Option<CommunityPolicy>,
    community_index: Option<&'a CommunityRoutingIndex>,
    community: Option<&'a mut CommunityDiagnostics>,
}

struct TraversalState<'a> {
    seed_ids: &'a BTreeSet<NodeId>,
    visited: BTreeSet<NodeId>,
    nodes: BTreeMap<NodeId, MemoryNode>,
    queue: VecDeque<State>,
    candidates: BTreeMap<NodeId, Candidate>,
    best_state: BTreeMap<(NodeId, usize), (f32, NodeId, Vec<PathStep>)>,
}

struct ActivationCompletion {
    stats: TraversalStats,
    database: DatabaseStats,
    adjacency_sources: Vec<NodeId>,
    timings: ActivationTimings,
    community: Option<CommunityDiagnostics>,
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
        self.activate_internal(query, options, None, None)
    }

    /// Runs opt-in deterministic feedback-aware activation over explicit relation feedback.
    ///
    /// This call never changes stored memories, relation weights, or feedback events. The default
    /// [`Repository::activate`] call remains plain activation.
    ///
    /// # Errors
    ///
    /// Returns a validation error for the query, activation options, or feedback policy, or a
    /// repository error while reading stored evidence.
    pub fn activate_with_feedback(
        &self,
        query: &str,
        options: &ActivationOptions,
        policy: &FeedbackPolicy,
    ) -> Result<ActivationReport, RepositoryError> {
        policy.validate()?;
        self.activate_internal(query, options, Some(*policy), None)
    }

    /// Runs experimental deterministic activation routed by current persisted communities.
    ///
    /// Community routing changes only candidate order. Scores and result paths continue to use raw
    /// stored relation weights. Missing or stale artifacts are rejected instead of silently
    /// falling back to plain traversal.
    ///
    /// # Errors
    ///
    /// Returns a validation error, a missing/stale artifact error, or a repository read error.
    pub fn activate_with_communities(
        &self,
        query: &str,
        options: &ActivationOptions,
        policy: &CommunityPolicy,
    ) -> Result<ActivationReport, RepositoryError> {
        policy.validate()?;
        self.activate_internal(query, options, None, Some(*policy))
    }

    fn activate_internal(
        &self,
        query: &str,
        options: &ActivationOptions,
        feedback_policy: Option<FeedbackPolicy>,
        community_policy: Option<CommunityPolicy>,
    ) -> Result<ActivationReport, RepositoryError> {
        let seed_started = Instant::now();
        options.validate()?;
        let terms = validate_query(query)?;
        let seed_limit = options.seed_limit.min(options.max_visited_nodes);
        let seeds = self.search_terms(&terms, seed_limit, None)?;
        let community_index = community_policy
            .map(|policy| self.community_routing_index(policy))
            .transpose()?;
        let mut community = community_policy
            .zip(community_index.as_ref())
            .map(|(policy, index)| CommunityDiagnostics::new(policy, index));
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
                community,
                ranked_candidates: Vec::new(),
            });
        }

        let graph_started = Instant::now();
        let mut adjacency_sources = Vec::new();
        let candidates = self.traverse_graph(
            seeds,
            options,
            TraversalContext {
                stats: &mut stats,
                database: &mut database,
                adjacency_sources: &mut adjacency_sources,
                feedback_policy,
                community_policy,
                community_index: community_index.as_ref(),
                community: community.as_mut(),
            },
        )?;
        let mut report = finish_activation(
            candidates,
            options.limit,
            ActivationCompletion {
                stats,
                database,
                adjacency_sources,
                timings: ActivationTimings {
                    seed_lookup,
                    graph: graph_started.elapsed(),
                },
                community,
            },
        )?;
        if let (Some(index), Some(diagnostics)) =
            (community_index.as_ref(), report.community.as_mut())
        {
            diagnostics.results = report
                .results
                .iter()
                .map(|result| community_path_provenance(result, index))
                .collect();
        }
        Ok(report)
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
        mut context: TraversalContext<'_>,
    ) -> Result<BTreeMap<NodeId, Candidate>, RepositoryError> {
        let seed_ids: BTreeSet<_> = seeds.iter().map(|seed| seed.node.id()).collect();
        let mut state = TraversalState {
            seed_ids: &seed_ids,
            visited: seed_ids.clone(),
            nodes: BTreeMap::new(),
            queue: VecDeque::new(),
            candidates: BTreeMap::new(),
            best_state: BTreeMap::new(),
        };

        for seed in seeds {
            let id = seed.node.id();
            state.nodes.insert(id, seed.node.clone());
            state.queue.push_back(State {
                node: id,
                seed: id,
                propagation: seed.score,
                steps: Vec::new(),
            });
            if options.include_seeds {
                consider_candidate(
                    &mut state.candidates,
                    seed.node,
                    seed.score * importance_factor(state.nodes[&id].importance()),
                    id,
                    Vec::new(),
                );
            }
        }

        while let Some(queue_state) = state.queue.pop_front() {
            let hop = queue_state.steps.len();
            if hop >= usize::from(options.hops)
                || context.stats.visited_edges >= options.max_visited_edges
                || context.stats.visited_nodes >= options.max_visited_nodes
            {
                continue;
            }
            let remaining_edges = options.max_visited_edges - context.stats.visited_edges;
            context.database.adjacency_queries += 1;
            context.adjacency_sources.push(queue_state.node);
            let ordered_relations =
                self.ordered_relations(queue_state.node, remaining_edges, &context)?;
            for (routed_rank, (raw_rank, relation, route)) in
                ordered_relations.into_iter().enumerate()
            {
                if let (Some(route), Some(index), Some(diagnostics)) = (
                    route,
                    context.community_index,
                    context.community.as_deref_mut(),
                ) {
                    diagnostics.record_edge(CommunityEdgeDiagnostic {
                        source: relation.source(),
                        target: relation.target(),
                        relation: relation.name().to_owned(),
                        raw_rank,
                        routed_rank,
                        route,
                        source_memberships: index.memberships_for(relation.source()),
                        target_memberships: index.memberships_for(relation.target()),
                    });
                }
                if !self.admit_relation(
                    &queue_state,
                    &relation,
                    options,
                    &mut context,
                    &mut state,
                )? {
                    break;
                }
            }
        }
        Ok(state.candidates)
    }

    fn ordered_relations(
        &self,
        source: NodeId,
        remaining_edges: usize,
        context: &TraversalContext<'_>,
    ) -> Result<Vec<(usize, Relation, Option<CommunityRoute>)>, RepositoryError> {
        let relations = if let Some(policy) = context.feedback_policy {
            self.feedback_ordered_relations(
                source,
                policy.candidate_scan_limit.min(MAX_VISITED_EDGES),
                policy,
            )?
        } else if let Some(policy) = context.community_policy {
            self.outgoing_relations(source, policy.candidate_scan_limit.min(MAX_VISITED_EDGES))?
        } else {
            self.outgoing_relations(source, remaining_edges)?
        };
        let mut ordered = relations
            .into_iter()
            .enumerate()
            .map(|(raw_rank, relation)| {
                let route = context
                    .community_index
                    .map(|index| community_route(index, &relation));
                (raw_rank, relation, route)
            })
            .collect::<Vec<_>>();
        if context.community_index.is_some() {
            ordered.sort_by_key(|(raw_rank, _, route)| (*route, *raw_rank));
        }
        Ok(ordered)
    }

    fn admit_relation(
        &self,
        queue_state: &State,
        relation: &Relation,
        options: &ActivationOptions,
        context: &mut TraversalContext<'_>,
        state: &mut TraversalState<'_>,
    ) -> Result<bool, RepositoryError> {
        context.stats.visited_edges += 1;
        let target = relation.target();
        if path_contains(queue_state, target) {
            return Ok(context.stats.visited_edges < options.max_visited_edges);
        }
        if !state.visited.contains(&target) {
            if context.stats.visited_nodes == options.max_visited_nodes {
                return Ok(false);
            }
            state.visited.insert(target);
            context.stats.visited_nodes += 1;
        }
        let node = if let Some(node) = state.nodes.get(&target) {
            node.clone()
        } else {
            context.database.traversal_node_reads += 1;
            let node = self.get_node(target)?.ok_or_else(|| {
                RepositoryError::InvalidDatabase(format!("relation target {target} is missing"))
            })?;
            state.nodes.insert(target, node.clone());
            node
        };
        let mut steps = queue_state.steps.clone();
        steps.push(PathStep::new(relation));
        let relation_weight = if let Some(policy) = context.feedback_policy {
            self.feedback_adjusted_weight(relation, policy)?
        } else {
            relation.weight()
        };
        let propagation = queue_state.propagation * relation_weight * 0.8;
        let score = propagation * importance_factor(node.importance());
        if options.include_seeds || !state.seed_ids.contains(&target) {
            consider_candidate(
                &mut state.candidates,
                node,
                score,
                queue_state.seed,
                steps.clone(),
            );
        }
        let key = (target, steps.len());
        if should_replace_state(
            state.best_state.get(&key),
            propagation,
            queue_state.seed,
            &steps,
        ) {
            state
                .best_state
                .insert(key, (propagation, queue_state.seed, steps.clone()));
            state.queue.push_back(State {
                node: target,
                seed: queue_state.seed,
                propagation,
                steps,
            });
        }
        Ok(context.stats.visited_edges < options.max_visited_edges)
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

    fn feedback_ordered_relations(
        &self,
        source: NodeId,
        limit: usize,
        policy: FeedbackPolicy,
    ) -> Result<Vec<Relation>, RepositoryError> {
        let mut relations = self.outgoing_relations(source, limit)?;
        let mut weighted = relations
            .iter()
            .map(|relation| {
                Ok((
                    self.feedback_adjusted_weight(relation, policy)?,
                    relation.clone(),
                ))
            })
            .collect::<Result<Vec<_>, RepositoryError>>()?;
        weighted.sort_by(|(left_weight, left), (right_weight, right)| {
            right_weight
                .total_cmp(left_weight)
                .then_with(|| right.weight().total_cmp(&left.weight()))
                .then_with(|| left.target().cmp(&right.target()))
                .then_with(|| left.name().cmp(right.name()))
        });
        relations = weighted
            .into_iter()
            .take(limit)
            .map(|(_, relation)| relation)
            .collect();
        Ok(relations)
    }

    fn feedback_adjusted_weight(
        &self,
        relation: &Relation,
        policy: FeedbackPolicy,
    ) -> Result<f32, RepositoryError> {
        let events = self.feedback_events_for_relation(
            relation.source(),
            relation.target(),
            relation.name(),
        )?;
        let mut positive = 0.0_f32;
        let mut negative = 0.0_f32;
        for event in events {
            let freshness = feedback_freshness(&event, policy);
            match event.kind() {
                crate::FeedbackKind::Positive => positive += event.strength() * freshness,
                crate::FeedbackKind::Negative => negative += event.strength() * freshness,
            }
        }
        let factor = (1.0 + positive * policy.positive_boost - negative * policy.negative_penalty)
            .clamp(policy.min_factor, policy.max_factor);
        Ok((relation.weight() * factor).clamp(0.0, 1.0))
    }
}

fn feedback_freshness(event: &crate::FeedbackEvent, policy: FeedbackPolicy) -> f32 {
    let Some(half_life) = policy.decay_half_life_ms else {
        return 1.0;
    };
    let age = policy.now_ms.saturating_sub(event.occurred_at_ms());
    let age = u64::try_from(age).map_or(u64::MAX, |value| value);
    let half_life = Duration::from_millis(half_life).as_secs_f32();
    let age = Duration::from_millis(age).as_secs_f32();
    half_life / (half_life + age)
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
    limit: usize,
    completion: ActivationCompletion,
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
    let ranked_candidates = ranked
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
    let results = ranked_candidates.iter().take(limit).cloned().collect();
    Ok(ActivationReport {
        results,
        stats: completion.stats,
        database: completion.database,
        adjacency_sources: completion.adjacency_sources,
        timings: completion.timings,
        community: completion.community,
        ranked_candidates,
    })
}

fn community_route(index: &CommunityRoutingIndex, relation: &Relation) -> CommunityRoute {
    match (
        index.memberships.get(&relation.source()),
        index.memberships.get(&relation.target()),
    ) {
        (Some(source), Some(target)) if source.0 == target.0 => CommunityRoute::Fine,
        (Some(source), Some(target)) if source.1 == target.1 => CommunityRoute::Coarse,
        _ => CommunityRoute::Boundary,
    }
}

fn community_path_provenance(
    result: &ActivationResult,
    index: &CommunityRoutingIndex,
) -> CommunityPathProvenance {
    let mut raw_path_nodes = vec![result.path.seed];
    raw_path_nodes.extend(result.path.steps.iter().map(|step| step.target));
    let memberships = raw_path_nodes
        .iter()
        .flat_map(|node| index.memberships_for(*node))
        .collect();
    CommunityPathProvenance {
        result: result.node.id(),
        raw_path_nodes,
        memberships,
    }
}
