//! Protocol-level acceptance tests for the stdio MCP adapter.

use std::{
    error::Error,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use memory3d_core::{ActivationOptions, Repository};
use serde_json::{Value, json};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

struct McpSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpSession {
    fn start(database: &std::path::Path) -> TestResult<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_memory3d-mcp"))
            .arg("--db")
            .arg(database)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().ok_or("server stdin was not piped")?;
        let stdout = child.stdout.take().ok_or("server stdout was not piped")?;
        let mut session = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        };
        let initialized = session.request(
            "initialize",
            &json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "memory3d-mcp-test", "version": "0"}
            }),
        )?;
        assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
        session.notify("notifications/initialized", &json!({}))?;
        Ok(session)
    }

    fn request(&mut self, method: &str, params: &Value) -> TestResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))?;
        let response = self.read_response()?;
        assert_eq!(response["id"], id);
        Ok(response)
    }

    fn notify(&mut self, method: &str, params: &Value) -> TestResult<()> {
        self.write(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
    }

    fn call_tool(&mut self, name: &str, arguments: &Value) -> TestResult<Value> {
        let response = self.request(
            "tools/call",
            &json!({
                "name": name,
                "arguments": arguments
            }),
        )?;
        Ok(response["result"]["structuredContent"].clone())
    }

    fn call_tool_raw(&mut self, name: &str, arguments: &Value) -> TestResult<Value> {
        self.request(
            "tools/call",
            &json!({
                "name": name,
                "arguments": arguments
            }),
        )
    }

    fn write(&mut self, message: &Value) -> TestResult<()> {
        serde_json::to_writer(&mut self.stdin, message)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_response(&mut self) -> TestResult<Value> {
        let mut line = String::new();
        let bytes = self.stdout.read_line(&mut line)?;
        if bytes == 0 {
            return Err("server closed stdout before responding".into());
        }
        Ok(serde_json::from_str(&line)?)
    }

    fn stop(mut self) -> TestResult<()> {
        drop(self.stdin);
        let status = self.child.wait()?;
        assert!(status.success());
        Ok(())
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn stdio_server_reopens_database_and_matches_core_activation() -> TestResult {
    let directory = tempdir()?;
    let database = directory.path().join("memory3d.memory3d");

    let mut first = McpSession::start(&database)?;
    let tools = first.request("tools/list", &json!({}))?;
    let tool_names = tools["result"]["tools"]
        .as_array()
        .ok_or("tools/list did not return an array")?
        .iter()
        .map(|tool| tool["name"].as_str().ok_or("tool name was not a string"))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        tool_names,
        vec!["activate", "add", "get", "health", "link", "search"]
    );
    let add_schema = tools["result"]["tools"]
        .as_array()
        .ok_or("tools/list did not return an array")?
        .iter()
        .find(|tool| tool["name"] == "add")
        .ok_or("add tool was missing")?;
    assert_eq!(
        add_schema["inputSchema"]["properties"]["text"]["maxLength"],
        memory3d_core::MAX_TEXT_BYTES
    );

    let seed = first.call_tool(
        "add",
        &json!({"kind": "component", "text": "Token refresh implementation", "importance": 1.0}),
    )?;
    let direct = first.call_tool(
        "add",
        &json!({"kind": "dependency", "text": "Authentication gateway", "importance": 1.0}),
    )?;
    let indirect = first.call_tool(
        "add",
        &json!({"kind": "incident", "text": "Mobile session loss incident", "importance": 1.0}),
    )?;
    first.call_tool(
        "link",
        &json!({"source": seed["id"], "target": direct["id"], "relation": "owns", "weight": 0.9}),
    )?;
    first.call_tool(
        "link",
        &json!({"source": direct["id"], "target": indirect["id"], "relation": "caused", "weight": 0.8}),
    )?;

    let invalid = first.call_tool_raw(
        "link",
        &json!({"source": seed["id"], "target": 999_u64, "relation": "missing"}),
    )?;
    assert_eq!(invalid["result"]["isError"], true);
    assert!(
        invalid["result"]["content"][0]["text"]
            .as_str()
            .ok_or("invalid response text was not a string")?
            .contains("does not exist")
    );
    first.stop()?;

    let mut second = McpSession::start(&database)?;
    let reopened = second.call_tool("get", &json!({"id": seed["id"]}))?;
    assert_eq!(reopened["node"]["text"], "Token refresh implementation");
    let activation = second.call_tool(
        "activate",
        &json!({
            "query": "token refresh",
            "hops": 3,
            "seed_limit": 1,
            "limit": 10,
            "max_visited_nodes": 64,
            "max_visited_edges": 128
        }),
    )?;
    let results = activation["results"]
        .as_array()
        .ok_or("activation results were not an array")?;
    let indirect_result = results
        .iter()
        .find(|result| result["node"]["id"] == indirect["id"])
        .ok_or("indirect activation result was missing")?;
    assert_eq!(indirect_result["hops"], 2);
    assert_eq!(
        indirect_result["path"]["steps"]
            .as_array()
            .ok_or("path steps were not an array")?
            .iter()
            .map(|step| step["relation"].as_str().ok_or("relation was not a string"))
            .collect::<Result<Vec<_>, _>>()?,
        vec!["owns", "caused"]
    );

    let repository = Repository::open(&database)?;
    let core = repository.activate(
        "token refresh",
        &ActivationOptions {
            hops: 3,
            seed_limit: 1,
            limit: 10,
            max_visited_nodes: 64,
            max_visited_edges: 128,
            include_seeds: false,
        },
    )?;
    assert_eq!(
        results
            .iter()
            .map(|result| result["node"]["id"].clone())
            .collect::<Vec<_>>(),
        core.results
            .iter()
            .map(|result| json!(result.node.id().get()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        activation["stats"]["visited_nodes"],
        core.stats.visited_nodes
    );
    assert_eq!(
        activation["stats"]["visited_edges"],
        core.stats.visited_edges
    );
    second.stop()?;
    Ok(())
}
