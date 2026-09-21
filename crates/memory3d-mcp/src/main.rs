//! Local stdio MCP server for `Memory3D`.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use memory3d_core::{
    ActivationOptions, ActivationReport, ApplyAuthorization, Coordinates, DatabaseStats,
    EvidenceBundle, EvidenceContextItem, EvidenceContextOptions, MAX_BUNDLE_BYTES,
    MAX_CONTEXT_BYTES, MAX_EVIDENCE_CANDIDATE_SCAN_LIMIT, MAX_EVIDENCE_ID_BYTES, MAX_HOPS,
    MAX_IDENTITY_BYTES, MAX_METADATA_BYTES, MAX_QUERY_BYTES, MAX_RELATION_NAME_BYTES, MAX_RESULTS,
    MAX_SEEDS, MAX_TEXT_BYTES, MAX_VISITED_EDGES, MAX_VISITED_NODES, MemoryKind, MemoryNode,
    Metadata, NewMemory, NewRelation, NodeId, Relation, Repository, SearchOptions, SearchResult,
    StoredEvidence, TraversalStats,
};
use rmcp::{
    Json, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const HELP: &str = "Memory3D MCP stdio server\n\nUsage:\n  memory3d-mcp --db PATH\n  memory3d-mcp -h|--help\n  memory3d-mcp -V|--version\n\nThe server exposes memory graph tools plus explicit evidence preview, apply, get, context, and rollback tools over stdio.\n";
const MCP_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2025_11_25;

type ToolResult<T> = Result<Json<T>, String>;

#[derive(Debug)]
struct CliError(String);

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CliError {}

#[derive(Clone)]
struct MemoryServer {
    database: PathBuf,
    repository: Arc<Mutex<Repository>>,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl MemoryServer {
    fn open(database: PathBuf) -> Result<Self, Box<dyn Error>> {
        let repository = Repository::open(&database)?;
        Ok(Self {
            database,
            repository: Arc::new(Mutex::new(repository)),
            tool_router: Self::tool_router(),
        })
    }

    fn with_repository<T>(
        &self,
        action: impl FnOnce(&Repository) -> Result<T, memory3d_core::RepositoryError>,
    ) -> Result<T, String> {
        let repository = self
            .repository
            .lock()
            .map_err(|_| "repository lock is poisoned".to_owned())?;
        action(&repository).map_err(|error| error.to_string())
    }

    fn with_repository_mut<T>(
        &self,
        action: impl FnOnce(&mut Repository) -> Result<T, memory3d_core::RepositoryError>,
    ) -> Result<T, String> {
        let mut repository = self
            .repository
            .lock()
            .map_err(|_| "repository lock is poisoned".to_owned())?;
        action(&mut repository).map_err(|error| error.to_string())
    }
}

#[tool_router]
impl MemoryServer {
    #[tool(description = "Report server, protocol, version, database, and node/relation counts.")]
    fn health(&self) -> ToolResult<HealthOutput> {
        self.with_repository(|repository| {
            Ok(HealthOutput {
                status: "ok".to_owned(),
                server: "memory3d-mcp".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                protocol_version: MCP_PROTOCOL_VERSION.as_str().to_owned(),
                sdk: "rmcp 2.0.0".to_owned(),
                transport: "stdio".to_owned(),
                database: self.database.display().to_string(),
                node_count: repository.list_nodes()?.len(),
                relation_count: repository.list_relations()?.len(),
            })
        })
        .map(Json)
    }

    #[tool(description = "Add one text memory node to the configured local database.")]
    fn add(&self, Parameters(input): Parameters<AddInput>) -> ToolResult<NodeOutput> {
        self.with_repository_mut(|repository| {
            let mut memory = NewMemory::new(
                MemoryKind::new(input.kind)?,
                input.text,
                input.importance.unwrap_or(0.5),
            )?;
            if let Some(metadata) = input.metadata {
                memory = memory.with_metadata(Metadata::new(metadata)?);
            }
            if let Some(coordinates) = input.coordinates {
                memory = memory.with_coordinates(coordinates.try_into()?);
            }
            repository.add_node(memory).map(NodeOutput::from)
        })
        .map(Json)
    }

    #[tool(description = "Create one directed weighted relation between existing memory nodes.")]
    fn link(&self, Parameters(input): Parameters<LinkInput>) -> ToolResult<RelationOutput> {
        self.with_repository_mut(|repository| {
            let relation = NewRelation::new(
                NodeId::new(input.source)?,
                NodeId::new(input.target)?,
                input.relation,
                input.weight.unwrap_or(1.0),
            )?;
            repository.add_relation(relation).map(RelationOutput::from)
        })
        .map(Json)
    }

    #[tool(description = "Get one memory node by stable ID.")]
    fn get(&self, Parameters(input): Parameters<GetInput>) -> ToolResult<GetOutput> {
        self.with_repository(|repository| {
            let id = NodeId::new(input.id)?;
            let node = repository.get_node(id)?.map(NodeOutput::from);
            Ok(GetOutput { node })
        })
        .map(Json)
    }

    #[tool(description = "Run deterministic lexical search over memory kind and text.")]
    fn search(&self, Parameters(input): Parameters<SearchInput>) -> ToolResult<SearchOutput> {
        self.with_repository(|repository| {
            let kind = input.kind.map(MemoryKind::new).transpose()?;
            let options = SearchOptions {
                limit: input.limit.unwrap_or(10),
                kind,
            };
            let results = repository
                .search(&input.query, &options)?
                .into_iter()
                .map(SearchResultOutput::from)
                .collect();
            Ok(SearchOutput {
                query: input.query,
                results,
            })
        })
        .map(Json)
    }

    #[tool(description = "Run bounded explainable graph activation from lexical seeds.")]
    fn activate(
        &self,
        Parameters(input): Parameters<ActivateInput>,
    ) -> ToolResult<ActivationOutput> {
        self.with_repository(|repository| {
            let defaults = ActivationOptions::default();
            let options = ActivationOptions {
                hops: input.hops.unwrap_or(defaults.hops),
                seed_limit: input.seed_limit.unwrap_or(defaults.seed_limit),
                limit: input.limit.unwrap_or(defaults.limit),
                max_visited_nodes: input
                    .max_visited_nodes
                    .unwrap_or(defaults.max_visited_nodes),
                max_visited_edges: input
                    .max_visited_edges
                    .unwrap_or(defaults.max_visited_edges),
                include_seeds: input.include_seeds.unwrap_or(false),
            };
            repository
                .activate(&input.query, &options)
                .map(|report| ActivationOutput::from_report(input.query, options, report))
        })
        .map(Json)
    }

    #[tool(
        description = "Validate a versioned textual evidence bundle and return its deterministic application diff without applying it."
    )]
    fn evidence_preview(
        &self,
        Parameters(input): Parameters<EvidenceBundleInput>,
    ) -> ToolResult<Value> {
        self.with_repository(|repository| {
            let bundle = EvidenceBundle::from_json(&input.bundle_json)?;
            let preview = repository.preview_evidence_bundle(&bundle)?;
            Ok(json!({
                "status":preview.status.as_str(),
                "idempotency_key":preview.idempotency_key,
                "fingerprint":preview.fingerprint,
                "evidence_ids":preview.evidence_ids,
                "projected_nodes":preview.projected_nodes,
                "projected_relations":preview.projected_relations
            }))
        })
        .map(Json)
    }

    #[tool(
        description = "Atomically apply one previewable evidence bundle under an explicit actor and policy; identical calls are idempotent."
    )]
    fn evidence_apply(
        &self,
        Parameters(input): Parameters<EvidenceApplyInput>,
    ) -> ToolResult<Value> {
        self.with_repository_mut(|repository| {
            let bundle = EvidenceBundle::from_json(&input.bundle_json)?;
            let authorization = ApplyAuthorization::new(input.actor, input.policy)?;
            let preview = repository.preview_evidence_bundle(&bundle)?;
            let report = repository.apply_evidence_bundle(&bundle, &authorization)?;
            Ok(json!({
                "preview_status":preview.status.as_str(),
                "bundle_id":report.bundle_id,
                "idempotency_key":report.idempotency_key,
                "replayed":report.replayed,
                "applied_at_ms":report.applied_at_ms,
                "items":report.items.iter().map(|item|json!({
                    "evidence_id":item.evidence_id,"node_id":item.node_id.get()
                })).collect::<Vec<_>>()
            }))
        })
        .map(Json)
    }

    #[tool(
        description = "Get one active evidence item with lifecycle, scope, provenance, references, and graph projection."
    )]
    fn evidence_get(&self, Parameters(input): Parameters<EvidenceGetInput>) -> ToolResult<Value> {
        self.with_repository(|repository| {
            Ok(json!({
                "evidence":repository.get_evidence_item(&input.evidence_id)?.as_ref().map(stored_evidence_json)
            }))
        })
        .map(Json)
    }

    #[tool(
        description = "Assemble conservative exact-scope settled evidence context with admitted/excluded items, conflicts, paths, and bounded work."
    )]
    fn evidence_context(
        &self,
        Parameters(input): Parameters<EvidenceContextInput>,
    ) -> ToolResult<Value> {
        self.with_repository(|repository| {
            let defaults = ActivationOptions::default();
            let activation = ActivationOptions {
                hops: input.hops.unwrap_or(defaults.hops),
                seed_limit: input.seed_limit.unwrap_or(defaults.seed_limit),
                limit: input.limit.unwrap_or(defaults.limit),
                max_visited_nodes: input.max_visited_nodes.unwrap_or(defaults.max_visited_nodes),
                max_visited_edges: input.max_visited_edges.unwrap_or(defaults.max_visited_edges),
                include_seeds: true,
            };
            let mut options =
                EvidenceContextOptions::settled(input.scope)?.with_activation(activation);
            if let Some(candidate_scan_limit) = input.candidate_scan_limit {
                options = options.with_candidate_scan_limit(candidate_scan_limit);
            }
            if let Some(max_bytes) = input.max_bytes {
                options = options.with_max_bytes(max_bytes)?;
            }
            let package = repository.assemble_evidence_context(&input.query, &options)?;
            Ok(json!({
                "query":package.query,
                "scope":package.scope,
                "abstained":package.abstained,
                "estimated_text_bytes":package.estimated_text_bytes,
                "min_relevance_score":package.min_relevance_score,
                "candidate_work":package.candidate_work,
                "ordinary_candidates_skipped":package.ordinary_candidates_skipped,
                "evidence_candidates_evaluated":package.evidence_candidates_evaluated,
                "candidate_scan_limit":package.candidate_scan_limit,
                "result_limit":package.result_limit,
                "stop_reason":package.stop_reason.as_str(),
                "traversal":{"seeds":package.traversal.seeds,"visited_nodes":package.traversal.visited_nodes,"visited_edges":package.traversal.visited_edges},
                "database_work":{"seed_queries":package.database.seed_queries,"seed_node_reads":package.database.seed_node_reads,"adjacency_queries":package.database.adjacency_queries,"traversal_node_reads":package.database.traversal_node_reads},
                "admitted":package.admitted.iter().map(context_item_json).collect::<Vec<_>>(),
                "excluded":package.excluded.iter().map(context_item_json).collect::<Vec<_>>()
            }))
        })
        .map(Json)
    }

    #[tool(
        description = "Explicitly compensate one active evidence bundle; dependencies block rollback and preserve every record."
    )]
    fn evidence_rollback(
        &self,
        Parameters(input): Parameters<EvidenceRollbackInput>,
    ) -> ToolResult<Value> {
        self.with_repository_mut(|repository| {
            let report = repository.rollback_evidence_bundle(&input.idempotency_key)?;
            Ok(json!({
                "idempotency_key":report.idempotency_key,
                "removed_items":report.removed_items,
                "removed_nodes":report.removed_nodes,
                "rolled_back_at_ms":report.rolled_back_at_ms
            }))
        })
        .map(Json)
    }
}

#[tool_handler]
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(MCP_PROTOCOL_VERSION)
            .with_server_info(Implementation::new("memory3d-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Use graph memory tools or the explicit preview/apply/context/rollback evidence workflow; no tool runs a model or silently applies candidates.",
            )
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct AddInput {
    #[schemars(length(min = 1, max = MAX_KIND_BYTES))]
    kind: String,
    #[schemars(length(min = 1, max = MAX_TEXT_BYTES))]
    text: String,
    #[schemars(range(min = 0.0, max = 1.0))]
    importance: Option<f32>,
    metadata: Option<Value>,
    coordinates: Option<CoordinatesInput>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CoordinatesInput {
    x: f64,
    y: f64,
    z: f64,
}

impl TryFrom<CoordinatesInput> for Coordinates {
    type Error = memory3d_core::ValidationError;

    fn try_from(value: CoordinatesInput) -> Result<Self, Self::Error> {
        Self::new(value.x, value.y, value.z)
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct LinkInput {
    #[schemars(range(min = 1))]
    source: u128,
    #[schemars(range(min = 1))]
    target: u128,
    #[schemars(length(min = 1, max = MAX_RELATION_NAME_BYTES))]
    relation: String,
    #[schemars(range(min = 0.0, max = 1.0))]
    weight: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GetInput {
    #[schemars(range(min = 1))]
    id: u128,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchInput {
    #[schemars(length(min = 1, max = MAX_QUERY_BYTES))]
    query: String,
    #[schemars(range(min = 1, max = MAX_RESULTS))]
    limit: Option<usize>,
    #[schemars(length(min = 1, max = MAX_KIND_BYTES))]
    kind: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ActivateInput {
    #[schemars(length(min = 1, max = MAX_QUERY_BYTES))]
    query: String,
    #[schemars(range(min = 0, max = MAX_HOPS))]
    hops: Option<u8>,
    #[schemars(range(min = 1, max = MAX_SEEDS))]
    seed_limit: Option<usize>,
    #[schemars(range(min = 1, max = MAX_RESULTS))]
    limit: Option<usize>,
    #[schemars(range(min = 1, max = MAX_VISITED_NODES))]
    max_visited_nodes: Option<usize>,
    #[schemars(range(min = 1, max = MAX_VISITED_EDGES))]
    max_visited_edges: Option<usize>,
    include_seeds: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EvidenceBundleInput {
    #[schemars(length(min = 1, max = MAX_BUNDLE_BYTES))]
    bundle_json: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EvidenceApplyInput {
    #[schemars(length(min = 1, max = MAX_BUNDLE_BYTES))]
    bundle_json: String,
    #[schemars(length(min = 1, max = MAX_IDENTITY_BYTES))]
    actor: String,
    #[schemars(length(min = 1, max = MAX_IDENTITY_BYTES))]
    policy: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EvidenceGetInput {
    #[schemars(length(min = 1, max = MAX_EVIDENCE_ID_BYTES))]
    evidence_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EvidenceContextInput {
    #[schemars(length(min = 1, max = MAX_QUERY_BYTES))]
    query: String,
    scope: BTreeMap<String, String>,
    #[schemars(range(min = 0, max = MAX_HOPS))]
    hops: Option<u8>,
    #[schemars(range(min = 1, max = MAX_SEEDS))]
    seed_limit: Option<usize>,
    #[schemars(range(min = 1, max = MAX_RESULTS))]
    limit: Option<usize>,
    #[schemars(range(min = 1, max = MAX_EVIDENCE_CANDIDATE_SCAN_LIMIT))]
    candidate_scan_limit: Option<usize>,
    #[schemars(range(min = 1, max = MAX_VISITED_NODES))]
    max_visited_nodes: Option<usize>,
    #[schemars(range(min = 1, max = MAX_VISITED_EDGES))]
    max_visited_edges: Option<usize>,
    #[schemars(range(min = 1, max = MAX_CONTEXT_BYTES))]
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EvidenceRollbackInput {
    #[schemars(length(min = 1, max = MAX_EVIDENCE_ID_BYTES))]
    idempotency_key: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct HealthOutput {
    status: String,
    server: String,
    version: String,
    protocol_version: String,
    sdk: String,
    transport: String,
    database: String,
    node_count: usize,
    relation_count: usize,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct NodeOutput {
    id: u128,
    kind: String,
    text: String,
    importance: f32,
    metadata: Value,
    coordinates: Option<CoordinatesOutput>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

impl From<MemoryNode> for NodeOutput {
    fn from(value: MemoryNode) -> Self {
        Self {
            id: value.id().get(),
            kind: value.kind().as_str().to_owned(),
            text: value.text().to_owned(),
            importance: value.importance(),
            metadata: Value::Object(value.metadata().as_object().clone()),
            coordinates: value.coordinates().map(CoordinatesOutput::from),
            created_at_ms: value.created_at_ms(),
            updated_at_ms: value.updated_at_ms(),
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct CoordinatesOutput {
    x: f64,
    y: f64,
    z: f64,
}

impl From<Coordinates> for CoordinatesOutput {
    fn from(value: Coordinates) -> Self {
        Self {
            x: value.x(),
            y: value.y(),
            z: value.z(),
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct RelationOutput {
    source: u128,
    target: u128,
    relation: String,
    weight: f32,
    reinforcement_count: u64,
    created_at_ms: i64,
    updated_at_ms: i64,
}

impl From<Relation> for RelationOutput {
    fn from(value: Relation) -> Self {
        Self {
            source: value.source().get(),
            target: value.target().get(),
            relation: value.name().to_owned(),
            weight: value.weight(),
            reinforcement_count: value.reinforcement_count(),
            created_at_ms: value.created_at_ms(),
            updated_at_ms: value.updated_at_ms(),
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct GetOutput {
    node: Option<NodeOutput>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct SearchOutput {
    query: String,
    results: Vec<SearchResultOutput>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct SearchResultOutput {
    node: NodeOutput,
    matched_terms: usize,
    score: f32,
}

impl From<SearchResult> for SearchResultOutput {
    fn from(value: SearchResult) -> Self {
        Self {
            node: value.node.into(),
            matched_terms: value.matched_terms,
            score: value.score,
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ActivationOutput {
    query: String,
    options: ActivationOptionsOutput,
    stats: TraversalStatsOutput,
    database: DatabaseStatsOutput,
    timings_us: ActivationTimingsOutput,
    results: Vec<ActivationResultOutput>,
}

impl ActivationOutput {
    fn from_report(query: String, options: ActivationOptions, report: ActivationReport) -> Self {
        Self {
            query,
            options: options.into(),
            stats: report.stats.into(),
            database: report.database.into(),
            timings_us: ActivationTimingsOutput {
                seed_lookup: duration_micros(report.timings.seed_lookup),
                graph: duration_micros(report.timings.graph),
            },
            results: report
                .results
                .into_iter()
                .map(|result| {
                    let hops = result.hops();
                    ActivationResultOutput {
                        node: result.node.into(),
                        score: result.score,
                        hops,
                        path: ActivationPathOutput {
                            seed: result.path.seed.get(),
                            steps: result
                                .path
                                .steps
                                .into_iter()
                                .map(|step| PathStepOutput {
                                    source: step.source.get(),
                                    relation: step.relation,
                                    weight: step.weight,
                                    target: step.target.get(),
                                })
                                .collect(),
                        },
                    }
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ActivationOptionsOutput {
    hops: u8,
    seed_limit: usize,
    limit: usize,
    max_visited_nodes: usize,
    max_visited_edges: usize,
    include_seeds: bool,
}

impl From<ActivationOptions> for ActivationOptionsOutput {
    fn from(value: ActivationOptions) -> Self {
        Self {
            hops: value.hops,
            seed_limit: value.seed_limit,
            limit: value.limit,
            max_visited_nodes: value.max_visited_nodes,
            max_visited_edges: value.max_visited_edges,
            include_seeds: value.include_seeds,
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct TraversalStatsOutput {
    seeds: usize,
    visited_nodes: usize,
    visited_edges: usize,
}

impl From<TraversalStats> for TraversalStatsOutput {
    fn from(value: TraversalStats) -> Self {
        Self {
            seeds: value.seeds,
            visited_nodes: value.visited_nodes,
            visited_edges: value.visited_edges,
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct DatabaseStatsOutput {
    seed_queries: usize,
    seed_node_reads: usize,
    adjacency_queries: usize,
    traversal_node_reads: usize,
}

impl From<DatabaseStats> for DatabaseStatsOutput {
    fn from(value: DatabaseStats) -> Self {
        Self {
            seed_queries: value.seed_queries,
            seed_node_reads: value.seed_node_reads,
            adjacency_queries: value.adjacency_queries,
            traversal_node_reads: value.traversal_node_reads,
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ActivationTimingsOutput {
    seed_lookup: u64,
    graph: u64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ActivationResultOutput {
    node: NodeOutput,
    score: f32,
    hops: usize,
    path: ActivationPathOutput,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct ActivationPathOutput {
    seed: u128,
    steps: Vec<PathStepOutput>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct PathStepOutput {
    source: u128,
    relation: String,
    weight: f32,
    target: u128,
}

fn stored_evidence_json(stored: &StoredEvidence) -> Value {
    let evidence = &stored.evidence;
    json!({
        "id":evidence.id(),
        "node":NodeOutput::from(stored.node.clone()),
        "kind":evidence.kind().as_str(),
        "text":evidence.text(),
        "source":evidence.source(),
        "producer":evidence.producer(),
        "scope":evidence.scope(),
        "lifecycle":evidence.lifecycle().as_str(),
        "created_at_ms":evidence.created_at_ms(),
        "observed_at_ms":evidence.observed_at_ms(),
        "derives_from":evidence.derives_from(),
        "conflict_group":evidence.conflict_group(),
        "conflicts_with":evidence.conflicts_with(),
        "supersedes":evidence.supersedes(),
        "decision":evidence.decision().map(|decision|json!({
            "actor":decision.actor(),"policy":decision.policy(),
            "supporting_evidence_ids":decision.supporting_evidence_ids(),
            "decided_at_ms":decision.decided_at_ms()
        })),
        "bundle_key":stored.bundle_key,
        "applied_at_ms":stored.applied_at_ms
    })
}

fn context_item_json(item: &EvidenceContextItem) -> Value {
    json!({
        "evidence":stored_evidence_json(&item.stored),
        "relevance_score":item.relevance_score,
        "selection_provenance":item.selection_provenance,
        "conflicts":item.conflicts,
        "superseded_by":item.superseded_by,
        "exclusion":item.exclusion.as_ref().map(memory3d_core::ContextExclusionReason::as_str),
        "path":{"seed":item.path.seed.get(),"steps":item.path.steps.iter().map(|step|json!({
            "source":step.source.get(),"relation":step.relation,"weight":step.weight,"target":step.target.get()
        })).collect::<Vec<_>>()}
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let Some(database) = parse_args()? else {
        return Ok(());
    };
    let server = MemoryServer::open(database)?;
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn parse_args() -> Result<Option<PathBuf>, Box<dyn Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{HELP}");
        return Ok(None);
    }
    if args.len() == 1 && (args[0] == "-V" || args[0] == "--version") {
        println!("memory3d-mcp {}", env!("CARGO_PKG_VERSION"));
        return Ok(None);
    }
    if args.len() != 2 || args[0] != "--db" {
        return Err(Box::new(CliError(format!("invalid arguments\n\n{HELP}"))));
    }
    let database = PathBuf::from(&args[1]);
    if database.as_os_str().is_empty() {
        return Err(Box::new(CliError(
            "database path must not be empty".to_owned(),
        )));
    }
    Ok(Some(database))
}

fn duration_micros(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).map_or(u64::MAX, |value| value)
}

const MAX_KIND_BYTES: usize = memory3d_core::MAX_KIND_BYTES;

#[allow(dead_code)]
const _: usize = MAX_METADATA_BYTES;
