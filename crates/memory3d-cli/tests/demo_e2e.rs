//! Task 04 subprocess acceptance test for the two-session demo.

use std::{error::Error, fs, path::Path, process::Command};

use memory3d_core::{MemoryKind, NewMemory, Repository};
use rusqlite::Connection;
use serde_json::Value;
use tempfile::tempdir;

type TestResult = Result<(), Box<dyn Error>>;

fn run_json(path: &Path, arguments: &[&str]) -> Result<Value, Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_memory3d-cli"))
        .args([
            "--db",
            path.to_str().ok_or("non-UTF-8 test path")?,
            "--json",
        ])
        .args(arguments)
        .output()?;
    if !output.status.success() {
        return Err(format!("CLI failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn run_failure(path: &Path, arguments: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_memory3d-cli"))
        .args(["--db", path.to_str().ok_or("non-UTF-8 test path")?])
        .args(arguments)
        .output()?;
    if output.status.success() {
        return Err("CLI unexpectedly succeeded".into());
    }
    Ok(String::from_utf8(output.stderr)?)
}

#[test]
fn ingest_and_recall_are_durable_deterministic_separate_processes() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("codebase.memory3d");

    let ingest = run_json(&path, &["demo", "ingest"])?;
    assert!(path.is_file());
    assert!(
        ingest["node_count"]
            .as_u64()
            .is_some_and(|count| count >= 25)
    );
    assert!(
        ingest["relation_count"]
            .as_u64()
            .is_some_and(|count| count >= 5)
    );

    let bytes_before_recall = fs::read(&path)?;
    let first = run_json(&path, &["demo", "recall"])?;
    let second = run_json(&path, &["demo", "recall"])?;
    assert_eq!(first["results"], second["results"]);
    assert_eq!(first["stats"], second["stats"]);
    assert_eq!(first["database"], second["database"]);
    assert_eq!(bytes_before_recall, fs::read(&path)?);
    assert_eq!(
        first["query"],
        "What should I remember before changing token refresh?"
    );
    assert!(first["stats"]["visited_nodes"].as_u64().unwrap_or(u64::MAX) <= 64);
    assert!(first["stats"]["visited_edges"].as_u64().unwrap_or(u64::MAX) <= 128);

    let results = first["results"]
        .as_array()
        .ok_or("results are not an array")?;
    assert!(results.iter().any(|result| {
        result["node"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Mobile session loss incident"))
    }));
    assert!(
        results
            .iter()
            .any(|result| result["hops"].as_u64().is_some_and(|hops| hops >= 2))
    );
    for result in results {
        assert!(result["score"].as_f64().is_some_and(f64::is_finite));
        let seed = result["path"]["seed"].as_u64().ok_or("missing seed")?;
        let steps = result["path"]["steps"].as_array().ok_or("missing steps")?;
        let endpoint = steps
            .last()
            .and_then(|step| step["target"].as_u64())
            .unwrap_or(seed);
        assert_eq!(endpoint, result["node"]["id"].as_u64().ok_or("missing ID")?);
    }

    let original = {
        let repository = Repository::open(&path)?;
        let nodes = repository.list_nodes()?;
        let relations = repository.list_relations()?;
        assert_eq!(
            nodes.len(),
            usize::try_from(ingest["node_count"].as_u64().unwrap_or_default())?
        );
        assert_eq!(
            relations.len(),
            usize::try_from(ingest["relation_count"].as_u64().unwrap_or_default())?
        );
        for result in results {
            for step in result["path"]["steps"].as_array().ok_or("missing steps")? {
                let source =
                    memory3d_core::NodeId::new(step["source"].as_u64().ok_or("source")?.into())?;
                let target =
                    memory3d_core::NodeId::new(step["target"].as_u64().ok_or("target")?.into())?;
                let name = step["relation"].as_str().ok_or("relation")?;
                let stored = repository
                    .get_relation(source, target, name)?
                    .ok_or("missing stored relation")?;
                let json_weight = step["weight"].as_f64().ok_or("weight")?;
                assert!((f64::from(stored.weight()) - json_weight).abs() < f64::EPSILON);
            }
        }
        nodes[0].clone()
    };
    {
        let mut reopened = Repository::open(&path)?;
        let appended = reopened.add_node(NewMemory::new(
            MemoryKind::new("note")?,
            "Appended after the demo process exited",
            0.5,
        )?)?;
        assert!(appended.id() > original.id());
        assert_eq!(
            reopened.get_node(original.id())?.ok_or("lost original")?,
            original
        );
    }

    let duplicate_error = run_failure(&path, &["demo", "ingest"])?;
    assert!(duplicate_error.contains("populated"));
    let missing_error = run_failure(&path, &["get", "--id", "999999"])?;
    assert!(missing_error.contains("does not exist"));
    let malformed_error = run_failure(&path, &["activate", "--query", "token", "--hops", "99"])?;
    assert!(malformed_error.contains("hops must be between"));
    Ok(())
}

#[test]
fn newer_schema_failure_does_not_alter_database() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("future.memory3d");
    run_json(&path, &["demo", "ingest"])?;
    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE memory3d_schema SET schema_version = 999 WHERE singleton = 1",
        [],
    )?;
    drop(connection);
    let before = fs::read(&path)?;
    let error = run_failure(&path, &["info"])?;
    assert!(error.contains("newer than supported"));
    assert_eq!(before, fs::read(path)?);
    Ok(())
}

#[test]
fn integrity_check_passes_demo_and_fails_clearly_without_mutating_invalid_file() -> TestResult {
    let directory = tempdir()?;
    let valid = directory.path().join("valid.memory3d");
    run_json(&valid, &["demo", "ingest"])?;
    let check = run_json(&valid, &["check"])?;
    assert_eq!(check["status"], "ok");
    assert_eq!(check["messages"], serde_json::json!([]));

    let invalid = directory.path().join("invalid.memory3d");
    fs::write(&invalid, b"not a sqlite database")?;
    let before = fs::read(&invalid)?;
    let error = run_failure(&invalid, &["check"])?;
    assert!(error.contains("integrity check failed"));
    assert_eq!(fs::read(invalid)?, before);
    Ok(())
}

#[test]
fn general_commands_cover_the_persistent_adapter_surface() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("commands.memory3d");
    let seed = run_json(
        &path,
        &[
            "add",
            "--kind",
            "component",
            "--text",
            "Token refresh component",
            "--importance",
            "1",
        ],
    )?;
    let target = run_json(
        &path,
        &["add", "--kind", "incident", "--text", "Mobile session loss"],
    )?;
    let seed_id = seed["node"]["id"].as_u64().ok_or("seed ID")?;
    let target_id = target["node"]["id"].as_u64().ok_or("target ID")?;
    run_json(
        &path,
        &[
            "link",
            "--source",
            &seed_id.to_string(),
            "--target",
            &target_id.to_string(),
            "--relation",
            "caused",
            "--weight",
            "0.8",
        ],
    )?;
    let fetched = run_json(&path, &["get", "--id", &target_id.to_string()])?;
    assert_eq!(fetched["node"]["text"], "Mobile session loss");
    let search = run_json(&path, &["search", "--query", "token refresh"])?;
    assert_eq!(search["results"][0]["node"]["id"], seed_id);
    let activation = run_json(
        &path,
        &["activate", "--query", "token refresh", "--seed-limit", "1"],
    )?;
    assert_eq!(activation["results"][0]["node"]["id"], target_id);
    assert_eq!(
        activation["results"][0]["path"]["steps"][0]["relation"],
        "caused"
    );
    let info = run_json(&path, &["info"])?;
    assert_eq!(info["node_count"], 2);
    assert_eq!(info["relation_count"], 1);
    Ok(())
}
