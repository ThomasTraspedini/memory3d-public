//! Command-line adapter for local `Memory3D` databases.

#![forbid(unsafe_code)]

mod fixture;

use std::{env, error::Error, fmt, fs, path::PathBuf, process::ExitCode};

use memory3d_core::{
    ActivationOptions, MemoryKind, MemoryNode, NewMemory, NewRelation, NodeId, Relation,
    Repository, SearchOptions,
};
use serde_json::{Value, json};

const HELP: &str = "Memory3D local associative memory\n\nUsage:\n  memory3d-cli [--db PATH] [--json] <COMMAND>\n\nCommands:\n  add --kind KIND --text TEXT [--importance NUMBER]\n  link --source ID --target ID --relation NAME [--weight NUMBER]\n  get --id ID\n  search --query TEXT [--limit NUMBER] [--kind KIND]\n  activate --query TEXT [activation options]\n  info\n  check\n  demo ingest [--replace]\n  demo recall\n\nActivation options:\n  --hops N --seed-limit N --limit N --max-visited-nodes N\n  --max-visited-edges N --include-seeds\n\nGlobal options:\n  --db PATH       Database file (default: memory3d.db)\n  --json          Stable machine-readable JSON output\n  -h, --help      Print help\n  -V, --version   Print version\n";

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
    reject_unknown(
        &global.args,
        &[
            "--query",
            "--hops",
            "--seed-limit",
            "--limit",
            "--max-visited-nodes",
            "--max-visited-edges",
        ],
        &["--include-seeds"],
    )?;
    activation_json(&global.db, &query, options, "activate")
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
    command: &str,
) -> Result<Value, Box<dyn Error>> {
    let repository = Repository::open(path)?;
    let report = repository.activate(query, &options)?;
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
            "include_seeds":options.include_seeds},
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
