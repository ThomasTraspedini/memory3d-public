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

fn evidence_ids(items: &Value) -> Result<Vec<&str>, Box<dyn Error>> {
    Ok(items
        .as_array()
        .ok_or("evidence items are not an array")?
        .iter()
        .map(|item| item["evidence"]["id"].as_str().ok_or("missing evidence ID"))
        .collect::<Result<Vec<_>, _>>()?)
}

fn exclusion_for<'a>(items: &'a Value, id: &str) -> Option<&'a str> {
    items.as_array()?.iter().find_map(|item| {
        (item["evidence"]["id"] == id)
            .then(|| item["exclusion"].as_str())
            .flatten()
    })
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
#[allow(clippy::too_many_lines)]
fn lifecycle_demo_reopens_with_paths_exclusions_conflict_abstention_and_bounds() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("lifecycle.memory3d");

    let ingest = run_json(&path, &["demo", "lifecycle", "ingest"])?;
    assert!(path.is_file());
    assert_eq!(ingest["ordinary_nodes"], 3);
    assert_eq!(ingest["evidence_items"], 8);
    assert_eq!(ingest["bundles"].as_array().map(Vec::len), Some(3));
    assert_eq!(
        ingest["authorization"]["policy"],
        "manual-environment-review-v1"
    );
    assert!(ingest["bundles"].as_array().is_some_and(|bundles| {
        bundles
            .iter()
            .all(|bundle| bundle["preview_status"] == "new" && bundle["replayed"] == false)
    }));

    let bytes_after_ingest = fs::read(&path)?;
    let recall = run_json(&path, &["demo", "lifecycle", "recall"])?;
    assert_eq!(bytes_after_ingest, fs::read(&path)?);
    assert_eq!(recall["persistence"]["node_count"], ingest["node_count"]);
    assert_eq!(
        recall["persistence"]["relation_count"],
        ingest["relation_count"]
    );
    assert_eq!(recall["persistence"]["evidence_items_present"], 8);

    let associative = &recall["associative_retrieval"];
    assert_eq!(associative["result"]["node"]["kind"], "environment");
    assert_eq!(associative["result"]["hops"], 1);
    assert_eq!(
        associative["result"]["path"]["steps"][0]["relation"],
        "connects_to"
    );
    assert_eq!(
        associative["result"]["path"]["steps"][0]["target"],
        associative["result"]["node"]["id"]
    );
    assert!(
        associative["traversal"]["visited_nodes"]
            .as_u64()
            .unwrap_or(u64::MAX)
            <= associative["bounds"]["max_visited_nodes"]
                .as_u64()
                .unwrap_or_default()
    );
    assert!(
        associative["traversal"]["visited_edges"]
            .as_u64()
            .unwrap_or(u64::MAX)
            <= associative["bounds"]["max_visited_edges"]
                .as_u64()
                .unwrap_or_default()
    );

    let settled = &recall["settled_context"];
    let admitted = evidence_ids(&settled["admitted"])?;
    assert!(admitted.contains(&"bedroom-calibrated-normal-v2"));
    assert!(admitted.contains(&"bedroom-monitor-v2"));
    assert_eq!(
        exclusion_for(&settled["excluded"], "bedroom-high-v1"),
        Some("superseded")
    );
    assert_eq!(
        exclusion_for(&settled["excluded"], "bedroom-inspect-v1"),
        Some("superseded")
    );
    assert_eq!(
        exclusion_for(&settled["excluded"], "nursery-high-v1"),
        Some("scope_mismatch")
    );
    assert_eq!(settled["abstained"], false);
    assert_eq!(settled["candidate_scan_limit"], 8);
    assert_eq!(settled["candidate_work"], 8);
    assert_eq!(settled["evidence_candidates_evaluated"], 6);
    assert_eq!(settled["ordinary_candidates_skipped"], 2);
    assert_eq!(settled["stop_reason"], "candidate_stream_exhausted");
    for item in settled["admitted"]
        .as_array()
        .ok_or("admitted is not an array")?
    {
        let steps = item["path"]["steps"]
            .as_array()
            .ok_or("missing evidence path")?;
        assert!(!steps.is_empty());
        assert_eq!(
            steps.last().ok_or("empty evidence path")?["target"],
            item["evidence"]["node"]["id"]
        );
    }

    let conflict = &recall["conflict_context"];
    assert_eq!(conflict["abstained"], true);
    assert!(conflict["admitted"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(conflict["candidate_scan_limit"], 4);
    assert_eq!(conflict["candidate_work"], 4);
    assert_eq!(conflict["evidence_candidates_evaluated"], 2);
    assert_eq!(conflict["ordinary_candidates_skipped"], 2);
    assert_eq!(
        exclusion_for(&conflict["excluded"], "bedroom-ventilation-closed-v1"),
        Some("conflict")
    );
    assert_eq!(
        exclusion_for(&conflict["excluded"], "bedroom-ventilation-open-v1"),
        Some("conflict")
    );
    assert!(conflict["excluded"].as_array().is_some_and(|items| {
        items.iter().all(|item| {
            item["conflicts"]
                .as_array()
                .is_some_and(|ids| !ids.is_empty())
        })
    }));

    let human = Command::new(env!("CARGO_BIN_EXE_memory3d-cli"))
        .args([
            "--db",
            path.to_str().ok_or("non-UTF-8 test path")?,
            "demo",
            "lifecycle",
            "recall",
        ])
        .output()?;
    assert!(human.status.success());
    let human = String::from_utf8(human.stdout)?;
    for label in [
        "Database reopened:",
        "ASSOCIATIVE RETRIEVAL",
        "path:",
        "admitted:",
        "excluded:",
        "reason=superseded",
        "reason=scope_mismatch requested={\"home\":\"demo-home\",\"room\":\"bedroom\"} candidate={\"home\":\"demo-home\",\"room\":\"nursery\"}",
        "reason=conflict",
        "abstained: true",
        "candidate scan:",
    ] {
        assert!(
            human.contains(label),
            "missing human-output label {label:?}"
        );
    }
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

#[test]
#[allow(clippy::too_many_lines)]
fn evidence_commands_share_preview_apply_context_get_and_rollback_core_behavior() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("evidence-cli.memory3d");
    let bundle_path = directory.path().join("bundle.json");
    fs::write(
        &bundle_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version":1,
            "idempotency_key":"cli-evidence-v1",
            "items":[{
                "id":"cli-filesystem-offset",
                "kind":"decision",
                "text":"Filesystem checkpoints use BYTE_OFFSET.",
                "source":"runbook:filesystem",
                "producer":"operator:cli-test",
                "scope":{"environment":"production","transport":"filesystem"},
                "lifecycle":"approved",
                "created_at_ms":1_700_000_000_000_i64,
                "decision":{
                    "actor":"operator:cli-test",
                    "policy":"manual-review-v1",
                    "decided_at_ms":1_700_000_000_100_i64
                }
            }]
        }))?,
    )?;
    let bundle = bundle_path.to_str().ok_or("non-UTF-8 bundle path")?;
    let preview = run_json(&path, &["evidence", "preview", "--bundle", bundle])?;
    assert_eq!(preview["status"], "new");
    assert_eq!(preview["projected_nodes"], 1);

    let applied = run_json(
        &path,
        &[
            "evidence",
            "apply",
            "--bundle",
            bundle,
            "--actor",
            "operator:cli-test",
            "--policy",
            "manual-review-v1",
        ],
    )?;
    assert_eq!(applied["replayed"], false);
    let replay = run_json(
        &path,
        &[
            "evidence",
            "apply",
            "--bundle",
            bundle,
            "--actor",
            "operator:cli-test",
            "--policy",
            "manual-review-v1",
        ],
    )?;
    assert_eq!(replay["replayed"], true);

    let stored = run_json(&path, &["evidence", "get", "--id", "cli-filesystem-offset"])?;
    assert_eq!(stored["evidence"]["lifecycle"], "approved");
    assert_eq!(stored["evidence"]["source"], "runbook:filesystem");

    let context = run_json(
        &path,
        &[
            "evidence",
            "context",
            "--query",
            "filesystem checkpoint offset",
            "--scope",
            r#"{"environment":"production","transport":"filesystem"}"#,
            "--candidate-scan-limit",
            "1",
        ],
    )?;
    assert_eq!(context["abstained"], false);
    assert_eq!(context["candidate_scan_limit"], 1);
    assert_eq!(context["result_limit"], 10);
    assert_eq!(context["candidate_work"], 1);
    assert_eq!(context["evidence_candidates_evaluated"], 1);
    assert_eq!(context["ordinary_candidates_skipped"], 0);
    assert_eq!(context["stop_reason"], "candidate_stream_exhausted");
    assert_eq!(
        context["admitted"][0]["evidence"]["id"],
        "cli-filesystem-offset"
    );
    assert_eq!(
        context["admitted"][0]["selection_provenance"],
        "bounded_lexical_seed_and_graph_activation"
    );

    let rollback = run_json(
        &path,
        &[
            "evidence",
            "rollback",
            "--idempotency-key",
            "cli-evidence-v1",
        ],
    )?;
    assert_eq!(rollback["removed_items"], 1);
    assert!(
        run_failure(&path, &["evidence", "get", "--id", "cli-filesystem-offset"])?
            .contains("does not exist")
    );
    Ok(())
}

#[test]
fn cli_feedback_event_changes_explicit_feedback_aware_activation() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("feedback.memory3d");
    let seed = run_json(
        &path,
        &[
            "add",
            "--kind",
            "component",
            "--text",
            "Feedback probe component",
            "--importance",
            "1",
        ],
    )?;
    let baseline = run_json(
        &path,
        &["add", "--kind", "evidence", "--text", "Baseline evidence"],
    )?;
    let boosted = run_json(
        &path,
        &["add", "--kind", "evidence", "--text", "Boosted evidence"],
    )?;
    let seed_id = seed["node"]["id"].as_u64().ok_or("seed ID")?;
    let baseline_id = baseline["node"]["id"].as_u64().ok_or("baseline ID")?;
    let boosted_id = boosted["node"]["id"].as_u64().ok_or("boosted ID")?;
    for (target, weight) in [(baseline_id, "0.6"), (boosted_id, "0.5")] {
        run_json(
            &path,
            &[
                "link",
                "--source",
                &seed_id.to_string(),
                "--target",
                &target.to_string(),
                "--relation",
                "reveals",
                "--weight",
                weight,
            ],
        )?;
    }

    let tight_args = [
        "activate",
        "--query",
        "feedback probe",
        "--seed-limit",
        "1",
        "--limit",
        "1",
        "--max-visited-nodes",
        "2",
        "--max-visited-edges",
        "1",
    ];
    let plain = run_json(&path, &tight_args)?;
    assert_eq!(plain["results"][0]["node"]["id"], baseline_id);

    let event = run_json(
        &path,
        &[
            "feedback",
            "--source",
            &seed_id.to_string(),
            "--target",
            &boosted_id.to_string(),
            "--relation",
            "reveals",
            "--kind",
            "positive",
            "--occurred-at-ms",
            "1000",
        ],
    )?;
    assert_eq!(event["event"]["kind"], "positive");
    assert_eq!(event["event"]["target"], boosted_id);

    let feedback = run_json(
        &path,
        &[
            "activate",
            "--query",
            "feedback probe",
            "--seed-limit",
            "1",
            "--limit",
            "1",
            "--max-visited-nodes",
            "2",
            "--max-visited-edges",
            "1",
            "--feedback-aware",
        ],
    )?;
    assert_eq!(feedback["results"][0]["node"]["id"], boosted_id);
    assert_eq!(
        feedback["options"]["feedback_policy"]["name"],
        "explicit-feedback-v1"
    );
    Ok(())
}
