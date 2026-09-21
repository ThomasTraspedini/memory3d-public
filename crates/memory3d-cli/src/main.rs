//! Command-line adapter for local `Memory3D` databases.

#![forbid(unsafe_code)]

mod fixture;

use std::{collections::BTreeMap, env, error::Error, fmt, fs, path::PathBuf, process::ExitCode};

use memory3d_core::{
    ActivationOptions, ApplyAuthorization, EvidenceBundle, EvidenceContextItem,
    EvidenceContextOptions, FeedbackKind, FeedbackPolicy, MemoryKind, MemoryNode, NewFeedbackEvent,
    NewMemory, NewRelation, NodeId, Relation, Repository, SearchOptions, StoredEvidence,
};
use serde_json::{Value, json};

const HELP: &str = "Memory3D local associative memory\n\nUsage:\n  memory3d-cli [--db PATH] [--json] <COMMAND>\n\nCommands:\n  add --kind KIND --text TEXT [--importance NUMBER]\n  link --source ID --target ID --relation NAME [--weight NUMBER]\n  get --id ID\n  search --query TEXT [--limit NUMBER] [--kind KIND]\n  activate --query TEXT [activation options]\n  feedback --source ID --target ID --relation NAME --kind positive|negative --occurred-at-ms N [--strength NUMBER]\n  evidence preview --bundle PATH\n  evidence apply --bundle PATH --actor IDENTITY --policy IDENTITY\n  evidence get --id EVIDENCE_ID\n  evidence context --query TEXT --scope JSON [activation options] [--candidate-scan-limit N] [--max-bytes N]\n  evidence rollback --idempotency-key KEY\n  info\n  check\n  demo ingest [--replace]\n  demo recall\n\nActivation options:\n  --hops N --seed-limit N --limit N --max-visited-nodes N\n  --max-visited-edges N --include-seeds --feedback-aware\n  --feedback-now-ms N --feedback-half-life-ms N --feedback-candidate-scan-limit N\n\nGlobal options:\n  --db PATH       Database file (default: memory3d.db)\n  --json          Stable machine-readable JSON output\n  -h, --help      Print help\n  -V, --version   Print version\n";

#[derive(Debug)]
struct CliError(String);

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CliError {}

struct Global {
    db: PathBuf,
    json: bool,
    command: String,
    args: Vec<String>,
}

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    if args.is_empty() || args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{HELP}");
        return Ok(());
    }
    if args.len() == 1 && (args[0] == "-V" || args[0] == "--version") {
        println!("memory3d-cli {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let global = parse_global(args)?;
    let output = match global.command.as_str() {
        "add" => add(&global)?,
        "link" => link(&global)?,
        "get" => get(&global)?,
        "search" => search(&global)?,
        "activate" => activate(&global)?,
        "feedback" => feedback(&global)?,
        "evidence" => evidence(&global)?,
        "info" => info(&global)?,
        "check" => check(&global)?,
        "demo" => demo(&global)?,
        command => return Err(cli(format!("unknown command {command:?}\n\n{HELP}"))),
    };
    print_output(&output, global.json)?;
    Ok(())
}

fn parse_global(args: &[String]) -> Result<Global, Box<dyn Error>> {
    let mut db = PathBuf::from("memory3d.db");
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db = PathBuf::from(args.get(index).ok_or_else(|| cli("--db requires a path"))?);
                index += 1;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            value if value.starts_with('-') => {
                return Err(cli(format!("unknown global option {value:?}")));
            }
            _ => break,
        }
    }
    let command = args
        .get(index)
        .ok_or_else(|| cli("a command is required"))?
        .clone();
    Ok(Global {
        db,
        json,
        command,
        args: args[(index + 1)..].to_vec(),
    })
}

fn add(global: &Global) -> Result<Value, Box<dyn Error>> {
    let kind = required(&global.args, "--kind")?;
    let text = required(&global.args, "--text")?;
    let importance = optional_parse(&global.args, "--importance")?.unwrap_or(0.5);
    reject_unknown(&global.args, &["--kind", "--text", "--importance"], &[])?;
    let mut repository = Repository::open(&global.db)?;
    let node = repository.add_node(NewMemory::new(MemoryKind::new(kind)?, text, importance)?)?;
    Ok(json!({"command":"add","database":global.db,"node":node_json(&node)}))
}

fn link(global: &Global) -> Result<Value, Box<dyn Error>> {
    let source = node_id(required_parse(&global.args, "--source")?)?;
    let target = node_id(required_parse(&global.args, "--target")?)?;
    let name = required(&global.args, "--relation")?;
    let weight = optional_parse(&global.args, "--weight")?.unwrap_or(1.0);
    reject_unknown(
        &global.args,
        &["--source", "--target", "--relation", "--weight"],
        &[],
    )?;
    let mut repository = Repository::open(&global.db)?;
    let relation = repository.add_relation(NewRelation::new(source, target, name, weight)?)?;
    Ok(json!({"command":"link","database":global.db,"relation":relation_json(&relation)}))
}

fn get(global: &Global) -> Result<Value, Box<dyn Error>> {
    let id = node_id(required_parse(&global.args, "--id")?)?;
    reject_unknown(&global.args, &["--id"], &[])?;
    let repository = Repository::open(&global.db)?;
    let node = repository
        .get_node(id)?
        .ok_or_else(|| cli(format!("node {id} does not exist")))?;
    Ok(json!({"command":"get","database":global.db,"node":node_json(&node)}))
}

fn search(global: &Global) -> Result<Value, Box<dyn Error>> {
    let query = required(&global.args, "--query")?;
    let limit = optional_parse(&global.args, "--limit")?.unwrap_or(10);
    let kind = optional(&global.args, "--kind")
        .map(MemoryKind::new)
        .transpose()?;
    reject_unknown(&global.args, &["--query", "--limit", "--kind"], &[])?;
    let repository = Repository::open(&global.db)?;
    let results = repository.search(&query, &SearchOptions { limit, kind })?;
    let results = results
        .iter()
        .map(|result| {
            json!({"node":node_json(&result.node),"matched_terms":result.matched_terms,"score":result.score})
        })
        .collect::<Vec<_>>();
    Ok(json!({"command":"search","database":global.db,"query":query,"results":results}))
}

fn activate(global: &Global) -> Result<Value, Box<dyn Error>> {
    let query = required(&global.args, "--query")?;
    let options = activation_options(&global.args)?;
    let feedback_policy = feedback_policy(&global.args)?;
    reject_unknown(
        &global.args,
        &[
            "--query",
            "--hops",
            "--seed-limit",
            "--limit",
            "--max-visited-nodes",
            "--max-visited-edges",
            "--feedback-now-ms",
            "--feedback-half-life-ms",
            "--feedback-candidate-scan-limit",
        ],
        &["--include-seeds", "--feedback-aware"],
    )?;
    activation_json(&global.db, &query, options, feedback_policy, "activate")
}

fn feedback(global: &Global) -> Result<Value, Box<dyn Error>> {
    let source = node_id(required_parse(&global.args, "--source")?)?;
    let target = node_id(required_parse(&global.args, "--target")?)?;
    let relation = required(&global.args, "--relation")?;
    let kind = parse_feedback_kind(&required(&global.args, "--kind")?)?;
    let occurred_at_ms = required_parse(&global.args, "--occurred-at-ms")?;
    let strength = optional_parse(&global.args, "--strength")?.unwrap_or(1.0);
    reject_unknown(
        &global.args,
        &[
            "--source",
            "--target",
            "--relation",
            "--kind",
            "--occurred-at-ms",
            "--strength",
        ],
        &[],
    )?;
    let mut repository = Repository::open(&global.db)?;
    let event = repository.record_relation_feedback(NewFeedbackEvent::new(
        source,
        target,
        relation,
        kind,
        strength,
        occurred_at_ms,
    )?)?;
    Ok(json!({
        "command":"feedback",
        "database":global.db,
        "event":{
            "id":event.id().get(),
            "source":event.source().get(),
            "target":event.target().get(),
            "relation":event.relation_name(),
            "kind":event.kind().as_str(),
            "strength":event.strength(),
            "occurred_at_ms":event.occurred_at_ms(),
            "created_at_ms":event.created_at_ms()
        }
    }))
}

fn evidence(global: &Global) -> Result<Value, Box<dyn Error>> {
    let subcommand = global
        .args
        .first()
        .ok_or_else(|| cli("evidence requires preview, apply, get, context, or rollback"))?;
    match subcommand.as_str() {
        "preview" => evidence_preview(global),
        "apply" => evidence_apply(global),
        "get" => evidence_get(global),
        "context" => evidence_context(global),
        "rollback" => evidence_rollback(global),
        _ => Err(cli(format!("unknown evidence command {subcommand:?}"))),
    }
}

fn evidence_preview(global: &Global) -> Result<Value, Box<dyn Error>> {
    let args = &global.args[1..];
    let bundle = read_bundle(&required(args, "--bundle")?)?;
    reject_unknown(args, &["--bundle"], &[])?;
    let repository = Repository::open(&global.db)?;
    let preview = repository.preview_evidence_bundle(&bundle)?;
    Ok(json!({
        "command":"evidence preview",
        "database":global.db,
        "status":preview.status.as_str(),
        "idempotency_key":preview.idempotency_key,
        "fingerprint":preview.fingerprint,
        "evidence_ids":preview.evidence_ids,
        "projected_nodes":preview.projected_nodes,
        "projected_relations":preview.projected_relations
    }))
}

fn evidence_apply(global: &Global) -> Result<Value, Box<dyn Error>> {
    let args = &global.args[1..];
    let bundle = read_bundle(&required(args, "--bundle")?)?;
    let authorization =
        ApplyAuthorization::new(required(args, "--actor")?, required(args, "--policy")?)?;
    reject_unknown(args, &["--bundle", "--actor", "--policy"], &[])?;
    let mut repository = Repository::open(&global.db)?;
    let preview = repository.preview_evidence_bundle(&bundle)?;
    let report = repository.apply_evidence_bundle(&bundle, &authorization)?;
    Ok(json!({
        "command":"evidence apply",
        "database":global.db,
        "preview_status":preview.status.as_str(),
        "bundle_id":report.bundle_id,
        "idempotency_key":report.idempotency_key,
        "replayed":report.replayed,
        "applied_at_ms":report.applied_at_ms,
        "items":report.items.iter().map(|item|json!({
            "evidence_id":item.evidence_id,"node_id":item.node_id.get()
        })).collect::<Vec<_>>()
    }))
}

fn evidence_get(global: &Global) -> Result<Value, Box<dyn Error>> {
    let args = &global.args[1..];
    let evidence_id = required(args, "--id")?;
    reject_unknown(args, &["--id"], &[])?;
    let repository = Repository::open(&global.db)?;
    let stored = repository
        .get_evidence_item(&evidence_id)?
        .ok_or_else(|| cli(format!("evidence {evidence_id:?} does not exist")))?;
    Ok(json!({
        "command":"evidence get",
        "database":global.db,
        "evidence":stored_evidence_json(&stored)
    }))
}

fn evidence_context(global: &Global) -> Result<Value, Box<dyn Error>> {
    let args = &global.args[1..];
    let query = required(args, "--query")?;
    let scope = parse_scope(&required(args, "--scope")?)?;
    let mut options =
        EvidenceContextOptions::settled(scope)?.with_activation(activation_options(args)?);
    if let Some(candidate_scan_limit) = optional_parse(args, "--candidate-scan-limit")? {
        options = options.with_candidate_scan_limit(candidate_scan_limit);
    }
    if let Some(max_bytes) = optional_parse(args, "--max-bytes")? {
        options = options.with_max_bytes(max_bytes)?;
    }
    reject_unknown(
        args,
        &[
            "--query",
            "--scope",
            "--max-bytes",
            "--candidate-scan-limit",
            "--hops",
            "--seed-limit",
            "--limit",
            "--max-visited-nodes",
            "--max-visited-edges",
        ],
        &["--include-seeds"],
    )?;
    let repository = Repository::open(&global.db)?;
    let package = repository.assemble_evidence_context(&query, &options)?;
    Ok(json!({
        "command":"evidence context",
        "database":global.db,
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
}

fn evidence_rollback(global: &Global) -> Result<Value, Box<dyn Error>> {
    let args = &global.args[1..];
    let key = required(args, "--idempotency-key")?;
    reject_unknown(args, &["--idempotency-key"], &[])?;
    let mut repository = Repository::open(&global.db)?;
    let report = repository.rollback_evidence_bundle(&key)?;
    Ok(json!({
        "command":"evidence rollback",
        "database":global.db,
        "idempotency_key":report.idempotency_key,
        "removed_items":report.removed_items,
        "removed_nodes":report.removed_nodes,
        "rolled_back_at_ms":report.rolled_back_at_ms
    }))
}

fn read_bundle(path: &str) -> Result<EvidenceBundle, Box<dyn Error>> {
    Ok(EvidenceBundle::from_json(&fs::read_to_string(path)?)?)
}

fn parse_scope(value: &str) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    serde_json::from_str(value).map_err(|error| {
        cli(format!(
            "--scope must be a JSON object of string selectors: {error}"
        ))
    })
}

fn stored_evidence_json(stored: &StoredEvidence) -> Value {
    let evidence = &stored.evidence;
    json!({
        "id":evidence.id(),
        "node":node_json(&stored.node),
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

fn info(global: &Global) -> Result<Value, Box<dyn Error>> {
    if !global.args.is_empty() {
        return Err(cli(format!("info takes no options: {:?}", global.args)));
    }
    database_info(&global.db, "info")
}

fn check(global: &Global) -> Result<Value, Box<dyn Error>> {
    if !global.args.is_empty() {
        return Err(cli(format!("check takes no options: {:?}", global.args)));
    }
    let report = Repository::check(&global.db)
        .map_err(|error| cli(format!("integrity check failed: {error}")))?;
    if !report.is_ok() {
        return Err(cli(format!(
            "integrity check failed: {}",
            report.messages().join("; ")
        )));
    }
    Ok(json!({
        "command":"check",
        "database":global.db,
        "status":"ok",
        "messages":report.messages()
    }))
}

fn demo(global: &Global) -> Result<Value, Box<dyn Error>> {
    let subcommand = global
        .args
        .first()
        .ok_or_else(|| cli("demo requires ingest or recall"))?;
    match subcommand.as_str() {
        "ingest" => demo_ingest(global),
        "recall" => {
            if global.args.len() != 1 {
                return Err(cli("demo recall takes no options"));
            }
            activation_json(
                &global.db,
                fixture::RECALL_QUERY,
                fixture::recall_options(),
                None,
                "demo recall",
            )
        }
        _ => Err(cli(format!("unknown demo command {subcommand:?}"))),
    }
}

fn demo_ingest(global: &Global) -> Result<Value, Box<dyn Error>> {
    let args = &global.args[1..];
    reject_unknown(args, &[], &["--replace"])?;
    let replace = flag(args, "--replace");
    if global.db.exists() && !replace {
        let existing = Repository::open(&global.db)?;
        if !existing.list_nodes()?.is_empty() || !existing.list_relations()?.is_empty() {
            return Err(cli(format!(
                "database {} is populated; pass --replace to replace it explicitly",
                global.db.display()
            )));
        }
    }
    let parent = global
        .db
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = global
        .db
        .file_name()
        .ok_or_else(|| cli("database path must name a file"))?
        .to_string_lossy();
    let temp = parent.join(format!(".{file_name}.ingest-{}", std::process::id()));
    if temp.exists() {
        fs::remove_file(&temp)?;
    }
    let ingest_result = (|| -> Result<(usize, usize), Box<dyn Error>> {
        let mut repository = Repository::open(&temp)?;
        let nodes = repository.add_nodes(&fixture::nodes()?)?;
        let relations = fixture::relations(&nodes)?;
        repository.add_relations(&relations)?;
        Ok((nodes.len(), relations.len()))
    })();
    let (node_count, relation_count) = match ingest_result {
        Ok(counts) => counts,
        Err(error) => {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
    };
    fs::rename(&temp, &global.db)?;
    Ok(json!({
        "command":"demo ingest",
        "database":global.db,
        "node_count":node_count,
        "relation_count":relation_count,
        "replaced":replace
    }))
}

fn activation_json(
    path: &PathBuf,
    query: &str,
    options: ActivationOptions,
    feedback_policy: Option<FeedbackPolicy>,
    command: &str,
) -> Result<Value, Box<dyn Error>> {
    let repository = Repository::open(path)?;
    let report = feedback_policy.map_or_else(
        || repository.activate(query, &options),
        |policy| repository.activate_with_feedback(query, &options, &policy),
    )?;
    let results = report
        .results
        .iter()
        .map(|result| {
            let steps = result.path.steps.iter().map(|step| json!({
                "source":step.source.get(),"relation":step.relation,"weight":step.weight,"target":step.target.get()
            })).collect::<Vec<_>>();
            json!({
                "node":node_json(&result.node),"score":result.score,"hops":result.hops(),
                "path":{"seed":result.path.seed.get(),"steps":steps}
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "command":command,"database":path,"query":query,
        "options":{"hops":options.hops,"seed_limit":options.seed_limit,"limit":options.limit,
            "max_visited_nodes":options.max_visited_nodes,"max_visited_edges":options.max_visited_edges,
            "include_seeds":options.include_seeds,
            "feedback_policy":feedback_policy.map(feedback_policy_json)},
        "stats":{"seeds":report.stats.seeds,"visited_nodes":report.stats.visited_nodes,
            "visited_edges":report.stats.visited_edges},
        "database":{"seed_queries":report.database.seed_queries,
            "seed_node_reads":report.database.seed_node_reads,
            "adjacency_queries":report.database.adjacency_queries,
            "traversal_node_reads":report.database.traversal_node_reads},
        "timings_us":{"seed_lookup":report.timings.seed_lookup.as_micros(),
            "graph":report.timings.graph.as_micros()},
        "results":results
    }))
}

fn database_info(path: &PathBuf, command: &str) -> Result<Value, Box<dyn Error>> {
    let repository = Repository::open(path)?;
    Ok(
        json!({"command":command,"database":path,"node_count":repository.list_nodes()?.len(),
        "relation_count":repository.list_relations()?.len(),"version":env!("CARGO_PKG_VERSION")}),
    )
}

fn node_json(node: &MemoryNode) -> Value {
    let coordinates = node
        .coordinates()
        .map(|point| json!({"x":point.x(),"y":point.y(),"z":point.z()}));
    json!({"id":node.id().get(),"kind":node.kind().as_str(),"text":node.text(),
        "importance":node.importance(),"metadata":node.metadata().as_object(),"coordinates":coordinates,
        "created_at_ms":node.created_at_ms(),"updated_at_ms":node.updated_at_ms()})
}

fn relation_json(relation: &Relation) -> Value {
    json!({"source":relation.source().get(),"target":relation.target().get(),"relation":relation.name(),
        "weight":relation.weight(),"reinforcement_count":relation.reinforcement_count(),
        "created_at_ms":relation.created_at_ms(),"updated_at_ms":relation.updated_at_ms()})
}

fn feedback_policy_json(policy: FeedbackPolicy) -> Value {
    json!({
        "name":"explicit-feedback-v1",
        "positive_boost":policy.positive_boost,
        "negative_penalty":policy.negative_penalty,
        "min_factor":policy.min_factor,
        "max_factor":policy.max_factor,
        "decay_half_life_ms":policy.decay_half_life_ms,
        "now_ms":policy.now_ms,
        "candidate_scan_limit":policy.candidate_scan_limit
    })
}

fn activation_options(args: &[String]) -> Result<ActivationOptions, Box<dyn Error>> {
    let defaults = ActivationOptions::default();
    Ok(ActivationOptions {
        hops: optional_parse(args, "--hops")?.unwrap_or(defaults.hops),
        seed_limit: optional_parse(args, "--seed-limit")?.unwrap_or(defaults.seed_limit),
        limit: optional_parse(args, "--limit")?.unwrap_or(defaults.limit),
        max_visited_nodes: optional_parse(args, "--max-visited-nodes")?
            .unwrap_or(defaults.max_visited_nodes),
        max_visited_edges: optional_parse(args, "--max-visited-edges")?
            .unwrap_or(defaults.max_visited_edges),
        include_seeds: flag(args, "--include-seeds"),
    })
}

fn feedback_policy(args: &[String]) -> Result<Option<FeedbackPolicy>, Box<dyn Error>> {
    if !flag(args, "--feedback-aware") {
        return Ok(None);
    }
    let mut policy = FeedbackPolicy::explicit_v1();
    if let Some(scan_limit) = optional_parse(args, "--feedback-candidate-scan-limit")? {
        policy.candidate_scan_limit = scan_limit;
    }
    if let Some(half_life) = optional_parse(args, "--feedback-half-life-ms")? {
        let now_ms = optional_parse(args, "--feedback-now-ms")?.unwrap_or(policy.now_ms);
        policy = policy.with_decay(now_ms, half_life);
    } else if let Some(now_ms) = optional_parse(args, "--feedback-now-ms")? {
        policy.now_ms = now_ms;
    }
    Ok(Some(policy))
}

fn required(args: &[String], option: &str) -> Result<String, Box<dyn Error>> {
    optional(args, option).ok_or_else(|| cli(format!("{option} is required")))
}

fn optional(args: &[String], option: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == option)
        .and_then(|index| args.get(index + 1))
        .filter(|value| !value.starts_with("--"))
        .cloned()
}

fn required_parse<T>(args: &[String], option: &str) -> Result<T, Box<dyn Error>>
where
    T: std::str::FromStr,
    T::Err: fmt::Display,
{
    optional_parse(args, option)?.ok_or_else(|| cli(format!("{option} is required")))
}

fn optional_parse<T>(args: &[String], option: &str) -> Result<Option<T>, Box<dyn Error>>
where
    T: std::str::FromStr,
    T::Err: fmt::Display,
{
    optional(args, option)
        .map(|value| {
            value
                .parse()
                .map_err(|error| cli(format!("invalid value for {option}: {error}")))
        })
        .transpose()
}

fn flag(args: &[String], option: &str) -> bool {
    args.iter().any(|arg| arg == option)
}

fn reject_unknown(args: &[String], valued: &[&str], flags: &[&str]) -> Result<(), Box<dyn Error>> {
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        if valued.contains(&argument) {
            if index + 1 >= args.len() || args[index + 1].starts_with("--") {
                return Err(cli(format!("{argument} requires a value")));
            }
            index += 2;
        } else if flags.contains(&argument) {
            index += 1;
        } else {
            return Err(cli(format!("unknown option or argument {argument:?}")));
        }
    }
    Ok(())
}

fn node_id(value: u128) -> Result<NodeId, Box<dyn Error>> {
    Ok(NodeId::new(value)?)
}

fn parse_feedback_kind(value: &str) -> Result<FeedbackKind, Box<dyn Error>> {
    match value {
        "positive" => Ok(FeedbackKind::Positive),
        "negative" => Ok(FeedbackKind::Negative),
        _ => Err(cli("feedback --kind must be positive or negative")),
    }
}

fn print_output(value: &Value, json_mode: bool) -> Result<(), Box<dyn Error>> {
    if json_mode {
        println!("{}", serde_json::to_string(&value)?);
    } else {
        println!("{}", human_output(value));
    }
    Ok(())
}

fn human_output(value: &Value) -> String {
    match value.get("command").and_then(Value::as_str) {
        Some("demo ingest" | "info") => format!(
            "Database: {}\nNodes: {}\nRelations: {}",
            value["database"].as_str().unwrap_or("?"),
            value["node_count"],
            value["relation_count"]
        ),
        Some("check") => format!(
            "Database: {}\nIntegrity: {}",
            value["database"].as_str().unwrap_or("?"),
            value["status"].as_str().unwrap_or("unknown")
        ),
        Some("activate" | "demo recall") => {
            let mut lines = vec![format!(
                "Recall: {} result(s), {} nodes / {} edges visited",
                value["results"].as_array().map_or(0, Vec::len),
                value["stats"]["visited_nodes"],
                value["stats"]["visited_edges"]
            )];
            if let Some(results) = value["results"].as_array() {
                for result in results {
                    let path = result["path"]["steps"]
                        .as_array()
                        .map(|steps| {
                            steps
                                .iter()
                                .map(|step| {
                                    format!(
                                        "-[{}]-> {}",
                                        step["relation"].as_str().unwrap_or("?"),
                                        step["target"]
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                    lines.push(format!(
                        "{} score={:.6} {} | {} {}",
                        result["node"]["id"],
                        result["score"].as_f64().unwrap_or(0.0),
                        result["node"]["text"].as_str().unwrap_or(""),
                        result["path"]["seed"],
                        path
                    ));
                }
            }
            lines.join("\n")
        }
        _ => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
    }
}

fn cli(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(CliError(message.into()))
}
