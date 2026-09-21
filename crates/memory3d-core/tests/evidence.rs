//! Task 27 evidence-envelope persistence, safety, and bounded-context contract.

use std::collections::BTreeMap;

use memory3d_core::{
    ActivationOptions, ApplyAuthorization, BundlePreviewStatus, ContextExclusionReason,
    ContextStopReason, EvidenceBundle, EvidenceContextOptions, EvidenceError, EvidenceLifecycle,
    MemoryKind, NewMemory, NewRelation, Repository, RepositoryError, ValidationError,
};
use serde_json::{Value, json};
use tempfile::tempdir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn scope(transport: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("environment".to_owned(), "production".to_owned()),
        ("transport".to_owned(), transport.to_owned()),
    ])
}

fn item(id: &str, text: &str, transport: &str, lifecycle: &str) -> Value {
    let decision = matches!(
        lifecycle,
        "corroborated" | "approved" | "contradicted" | "superseded" | "rejected"
    )
    .then(|| {
        json!({
            "actor":"operator:alice",
            "policy":"deployment-review-v1",
            "decided_at_ms":1_700_000_000_100_i64
        })
    });
    let mut value = json!({
        "id":id,
        "kind":"decision",
        "text":text,
        "source":"runbook:transport",
        "producer":"tool:manual-review",
        "scope":{"environment":"production","transport":transport},
        "lifecycle":lifecycle,
        "created_at_ms":1_700_000_000_000_i64,
        "observed_at_ms":1_700_000_000_050_i64
    });
    if let Some(decision) = decision {
        value["decision"] = decision;
    }
    value
}

#[allow(clippy::needless_pass_by_value)]
fn bundle(key: &str, items: Vec<Value>) -> Result<EvidenceBundle, EvidenceError> {
    EvidenceBundle::from_json(
        &serde_json::to_string(&json!({
            "version":1,
            "idempotency_key":key,
            "items":items
        }))
        .map_err(|error| EvidenceError::InvalidJson(error.to_string()))?,
    )
}

fn authorization() -> Result<ApplyAuthorization, EvidenceError> {
    ApplyAuthorization::new("operator:alice", "manual-review-v1")
}

#[test]
fn context_scans_ranked_candidates_before_activation_limit_and_counts_work() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("candidate-stream.memory3d");
    let mut repository = Repository::open(&path)?;
    repository.add_node(NewMemory::new(
        MemoryKind::new("note")?,
        "signal 00 ordinary",
        1.0,
    )?)?;
    repository.apply_evidence_bundle(
        &bundle(
            "candidate-stream-v1",
            vec![
                item("mismatch", "signal 01 mismatch", "kafka", "observed"),
                item("eligible", "signal 02 eligible", "filesystem", "observed"),
            ],
        )?,
        &authorization()?,
    )?;
    let activation = ActivationOptions {
        limit: 1,
        include_seeds: true,
        ..ActivationOptions::default()
    };
    assert_eq!(repository.activate("signal", &activation)?.results.len(), 1);
    let context = repository.assemble_evidence_context(
        "signal",
        &EvidenceContextOptions::settled(scope("filesystem"))?
            .with_activation(activation)
            .with_candidate_scan_limit(3),
    )?;
    assert_eq!(context.candidate_work, 3);
    assert_eq!(context.ordinary_candidates_skipped, 1);
    assert_eq!(context.evidence_candidates_evaluated, 2);
    assert_eq!(context.admitted[0].stored.evidence.id(), "eligible");
    assert_eq!(context.excluded.len(), 1);
    assert_eq!(context.stop_reason, ContextStopReason::ResultLimitReached);
    let repeated = repository.assemble_evidence_context(
        "signal",
        &EvidenceContextOptions::settled(scope("filesystem"))?
            .with_activation(activation)
            .with_candidate_scan_limit(3),
    )?;
    assert_eq!(context.admitted, repeated.admitted);
    assert_eq!(context.excluded, repeated.excluded);
    drop(repository);
    let reopened = Repository::open(&path)?;
    let after_reopen = reopened.assemble_evidence_context(
        "signal",
        &EvidenceContextOptions::settled(scope("filesystem"))?
            .with_activation(activation)
            .with_candidate_scan_limit(3),
    )?;
    assert_eq!(context.admitted, after_reopen.admitted);
    assert_eq!(context.excluded, after_reopen.excluded);
    Ok(())
}

#[test]
fn context_scan_bounds_and_byte_overflow_are_deterministic() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("bounds.memory3d"))?;
    repository.apply_evidence_bundle(
        &bundle(
            "bounds-v1",
            vec![
                item("large", "q a long item", "filesystem", "observed"),
                item("small", "q b", "filesystem", "observed"),
            ],
        )?,
        &authorization()?,
    )?;
    let context = repository.assemble_evidence_context(
        "q",
        &EvidenceContextOptions::settled(scope("filesystem"))?
            .with_max_bytes(3)?
            .with_candidate_scan_limit(2),
    )?;
    assert_eq!(context.admitted[0].stored.evidence.id(), "small");
    assert!(context.excluded.iter().any(|item| {
        item.stored.evidence.id() == "large"
            && item.exclusion == Some(ContextExclusionReason::SizeLimit)
    }));
    assert_eq!(context.stop_reason, ContextStopReason::ByteLimitReached);
    for limit in [0, 1_001] {
        assert!(matches!(
            repository.assemble_evidence_context(
                "q",
                &EvidenceContextOptions::settled(scope("filesystem"))?
                    .with_candidate_scan_limit(limit),
            ),
            Err(RepositoryError::Validation(ValidationError::OutOfRange {
                field: "evidence_candidate_scan_limit",
                min: 1,
                max: 1_000
            }))
        ));
    }
    Ok(())
}

#[test]
fn context_candidate_bound_distinguishes_starvation_exhaustion_and_result_limit() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("starvation.memory3d"))?;
    let mut items = Vec::new();
    for rank in 0..10 {
        items.push(item(
            &format!("excluded-{rank:02}"),
            &format!("signal {rank:02} excluded"),
            "kafka",
            "observed",
        ));
    }
    for rank in 10..13 {
        items.push(item(
            &format!("eligible-{rank:02}"),
            &format!("signal {rank:02} eligible"),
            "filesystem",
            "observed",
        ));
    }
    repository.apply_evidence_bundle(&bundle("starvation-v1", items)?, &authorization()?)?;
    let activation = ActivationOptions {
        seed_limit: 32,
        include_seeds: true,
        ..ActivationOptions::default()
    };
    let starved = repository.assemble_evidence_context(
        "signal",
        &EvidenceContextOptions::settled(scope("filesystem"))?
            .with_activation(activation)
            .with_candidate_scan_limit(10),
    )?;
    assert!(starved.abstained);
    assert_eq!(starved.excluded.len(), 10);
    assert_eq!(
        starved.stop_reason,
        ContextStopReason::CandidateScanLimitReached
    );

    let limited = repository.assemble_evidence_context(
        "signal",
        &EvidenceContextOptions::settled(scope("filesystem"))?
            .with_activation(activation)
            .with_candidate_scan_limit(13)
            .with_result_limit(2),
    )?;
    assert_eq!(limited.admitted.len(), 2);
    assert_eq!(limited.candidate_work, 12);
    assert_eq!(limited.stop_reason, ContextStopReason::ResultLimitReached);

    let mut ordinary = Repository::open(directory.path().join("ordinary-only.memory3d"))?;
    ordinary.add_node(NewMemory::new(
        MemoryKind::new("note")?,
        "plain signal",
        1.0,
    )?)?;
    let exhausted = ordinary.assemble_evidence_context(
        "signal",
        &EvidenceContextOptions::settled(scope("filesystem"))?.with_candidate_scan_limit(1),
    )?;
    assert!(exhausted.abstained);
    assert_eq!(exhausted.ordinary_candidates_skipped, 1);
    assert!(exhausted.excluded.is_empty());
    assert_eq!(
        exhausted.stop_reason,
        ContextStopReason::CandidateStreamExhausted
    );
    Ok(())
}

#[test]
fn traversal_exhaustion_is_reported_separately_from_candidate_scan_exhaustion() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("traversal-bound.memory3d"))?;
    let seed = repository.add_node(NewMemory::new(
        MemoryKind::new("note")?,
        "signal seed",
        1.0,
    )?)?;
    let applied = repository.apply_evidence_bundle(
        &bundle(
            "traversal-bound-v1",
            vec![item(
                "target",
                "reachable evidence",
                "filesystem",
                "observed",
            )],
        )?,
        &authorization()?,
    )?;
    repository.add_relation(NewRelation::new(
        seed.id(),
        applied.items[0].node_id,
        "supports",
        1.0,
    )?)?;
    let context = repository.assemble_evidence_context(
        "signal",
        &EvidenceContextOptions::settled(scope("filesystem"))?.with_activation(ActivationOptions {
            hops: 1,
            max_visited_edges: 1,
            include_seeds: true,
            ..ActivationOptions::default()
        }),
    )?;
    assert_eq!(context.traversal.visited_edges, 1);
    assert_eq!(context.candidate_scan_limit, 100);
    assert_eq!(
        context.stop_reason,
        ContextStopReason::CandidateStreamExhausted
    );
    assert_eq!(context.admitted.len(), 1);
    Ok(())
}

#[test]
fn preview_apply_reopen_and_idempotent_replay_preserve_stable_ids() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("evidence.memory3d");
    let bundle = bundle(
        "bundle-reopen-v1",
        vec![item(
            "filesystem-offset-v1",
            "Filesystem checkpoints use BYTE_OFFSET.",
            "filesystem",
            "approved",
        )],
    )?;
    let mut repository = Repository::open(&path)?;
    let preview = repository.preview_evidence_bundle(&bundle)?;
    assert_eq!(preview.status, BundlePreviewStatus::New);
    assert_eq!(preview.projected_nodes, 1);
    let first = repository.apply_evidence_bundle(&bundle, &authorization()?)?;
    assert!(!first.replayed);
    assert_eq!(repository.list_nodes()?.len(), 1);
    drop(repository);

    let mut reopened = Repository::open(&path)?;
    let stored = reopened
        .get_evidence_item("filesystem-offset-v1")?
        .ok_or("evidence missing after reopen")?;
    assert_eq!(stored.node.id(), first.items[0].node_id);
    assert_eq!(stored.evidence.lifecycle(), EvidenceLifecycle::Approved);
    assert_eq!(stored.evidence.source(), "runbook:transport");
    assert_eq!(stored.evidence.producer(), "tool:manual-review");
    let replay = reopened.apply_evidence_bundle(&bundle, &authorization()?)?;
    assert!(replay.replayed);
    assert_eq!(replay.bundle_id, first.bundle_id);
    assert_eq!(replay.items, first.items);
    assert_eq!(reopened.list_nodes()?.len(), 1);
    Ok(())
}

#[test]
fn invalid_duplicate_and_conflicting_idempotency_inputs_are_typed() -> TestResult {
    let duplicate = bundle(
        "duplicate-v1",
        vec![
            item("same", "first", "filesystem", "observed"),
            item("same", "second", "filesystem", "observed"),
        ],
    );
    assert!(matches!(
        duplicate,
        Err(EvidenceError::DuplicateEvidenceId(id)) if id == "same"
    ));

    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("idempotency.memory3d"))?;
    let first = bundle(
        "same-key",
        vec![item("one", "first payload", "filesystem", "observed")],
    )?;
    repository.apply_evidence_bundle(&first, &authorization()?)?;
    let changed = bundle(
        "same-key",
        vec![item("two", "different payload", "filesystem", "observed")],
    )?;
    assert!(matches!(
        repository.preview_evidence_bundle(&changed),
        Err(RepositoryError::IdempotencyConflict(key)) if key == "same-key"
    ));
    assert_eq!(repository.list_nodes()?.len(), 1);
    Ok(())
}

#[test]
fn missing_reference_fails_atomically_without_partial_nodes() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("atomic.memory3d"))?;
    let mut derived = item("derived", "Derived assertion", "filesystem", "candidate");
    derived["derives_from"] = json!(["missing-source"]);
    let bundle = bundle("atomic-v1", vec![derived])?;
    assert!(matches!(
        repository.apply_evidence_bundle(&bundle, &authorization()?),
        Err(RepositoryError::EvidenceReferenceNotFound { target, .. }) if target == "missing-source"
    ));
    assert!(repository.list_nodes()?.is_empty());
    assert!(repository.get_evidence_item("derived")?.is_none());
    Ok(())
}

#[test]
fn conflicts_and_supersession_remain_visible_and_are_not_settled() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("status.memory3d"))?;
    let mut kafka = item(
        "kafka-offset",
        "Kafka checkpoints use KAFKA_OFFSET.",
        "kafka",
        "approved",
    );
    let mut kafka_competing = item(
        "kafka-offset-competing",
        "Kafka checkpoints use LOG_POSITION.",
        "kafka",
        "observed",
    );
    kafka["conflict_group"] = json!("kafka-offset-policy");
    kafka["conflicts_with"] = json!(["kafka-offset-competing"]);
    kafka_competing["conflict_group"] = json!("kafka-offset-policy");
    repository.apply_evidence_bundle(
        &bundle("conflict-v1", vec![kafka, kafka_competing])?,
        &authorization()?,
    )?;

    let mut newer = item(
        "filesystem-offset-v2",
        "Filesystem checkpoints use BYTE_CURSOR_V2.",
        "filesystem",
        "approved",
    );
    let old = item(
        "filesystem-offset-v1",
        "Filesystem checkpoints use BYTE_CURSOR_V1.",
        "filesystem",
        "approved",
    );
    repository.apply_evidence_bundle(&bundle("old-v1", vec![old])?, &authorization()?)?;
    newer["supersedes"] = json!(["filesystem-offset-v1"]);
    newer["decision"]["supporting_evidence_ids"] = json!(["filesystem-offset-v1"]);
    repository.apply_evidence_bundle(&bundle("new-v2", vec![newer])?, &authorization()?)?;

    let kafka_context = repository.assemble_evidence_context(
        "Kafka offset checkpoint",
        &EvidenceContextOptions::settled(scope("kafka"))?,
    )?;
    assert!(kafka_context.abstained);
    assert!(kafka_context.excluded.iter().any(|candidate| {
        candidate.exclusion == Some(ContextExclusionReason::Conflict)
            && !candidate.conflicts.is_empty()
    }));

    let filesystem_context = repository.assemble_evidence_context(
        "Filesystem checkpoint cursor",
        &EvidenceContextOptions::settled(scope("filesystem"))?,
    )?;
    assert!(
        filesystem_context
            .admitted
            .iter()
            .any(|candidate| candidate.stored.evidence.id() == "filesystem-offset-v2")
    );
    assert!(filesystem_context.excluded.iter().any(|candidate| {
        candidate.stored.evidence.id() == "filesystem-offset-v1"
            && candidate.exclusion == Some(ContextExclusionReason::Superseded)
            && candidate.superseded_by == ["filesystem-offset-v2"]
    }));
    assert!(
        repository
            .get_evidence_item("filesystem-offset-v1")?
            .is_some()
    );
    Ok(())
}

#[test]
fn exact_scope_regression_abstains_instead_of_settling_kafka_for_filesystem() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("scope.memory3d"))?;
    repository.apply_evidence_bundle(
        &bundle(
            "scope-v1",
            vec![item(
                "kafka-only",
                "The checkpoint value is KAFKA_OFFSET.",
                "kafka",
                "approved",
            )],
        )?,
        &authorization()?,
    )?;
    let context = repository.assemble_evidence_context(
        "What checkpoint value should filesystem transport use?",
        &EvidenceContextOptions::settled(scope("filesystem"))?,
    )?;
    assert!(context.abstained);
    assert!(context.admitted.is_empty());
    assert_eq!(context.excluded.len(), 1);
    assert_eq!(
        context.excluded[0].exclusion,
        Some(ContextExclusionReason::ScopeMismatch)
    );
    assert_eq!(
        context.excluded[0].stored.evidence.text(),
        "The checkpoint value is KAFKA_OFFSET."
    );
    assert_eq!(context.candidate_work, 1);
    assert_eq!(
        context.stop_reason,
        ContextStopReason::CandidateStreamExhausted
    );
    assert!(context.traversal.visited_nodes <= 1_000);
    Ok(())
}

#[test]
fn context_byte_limit_excludes_overflow_with_explicit_status_and_path() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("bounded.memory3d"))?;
    repository.apply_evidence_bundle(
        &bundle(
            "bounded-v1",
            vec![item(
                "bounded-item",
                "bounded context payload",
                "filesystem",
                "observed",
            )],
        )?,
        &authorization()?,
    )?;
    let options = EvidenceContextOptions::settled(scope("filesystem"))?.with_max_bytes(4)?;
    let context = repository.assemble_evidence_context("bounded payload", &options)?;
    assert!(context.abstained);
    assert_eq!(
        context.excluded[0].exclusion,
        Some(ContextExclusionReason::SizeLimit)
    );
    assert_eq!(
        context.excluded[0].path.seed,
        context.excluded[0].stored.node.id()
    );
    assert_eq!(context.estimated_text_bytes, 0);
    Ok(())
}

#[test]
fn explicit_rollback_compensates_once_and_preserves_ledger() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("rollback.memory3d");
    let bundle = bundle(
        "rollback-v1",
        vec![item(
            "rollback-item",
            "temporary reviewed evidence",
            "filesystem",
            "observed",
        )],
    )?;
    let mut repository = Repository::open(&path)?;
    repository.apply_evidence_bundle(&bundle, &authorization()?)?;
    let rollback = repository.rollback_evidence_bundle("rollback-v1")?;
    assert_eq!(rollback.removed_items, 1);
    assert_eq!(rollback.removed_nodes, 1);
    assert!(repository.get_evidence_item("rollback-item")?.is_none());
    assert!(repository.list_nodes()?.is_empty());
    assert!(matches!(
        repository.apply_evidence_bundle(&bundle, &authorization()?),
        Err(RepositoryError::EvidenceBundleRolledBack(key)) if key == "rollback-v1"
    ));
    drop(repository);
    let reopened = Repository::open(path)?;
    assert!(reopened.get_evidence_item("rollback-item")?.is_none());
    Ok(())
}

#[test]
fn rollback_is_blocked_when_later_bundle_depends_on_raw_evidence() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("rollback-block.memory3d"))?;
    repository.apply_evidence_bundle(
        &bundle(
            "source-bundle",
            vec![item(
                "raw-observation",
                "Raw observation",
                "filesystem",
                "observed",
            )],
        )?,
        &authorization()?,
    )?;
    let mut derived = item(
        "derived-candidate",
        "Derived candidate",
        "filesystem",
        "candidate",
    );
    derived["derives_from"] = json!(["raw-observation"]);
    repository.apply_evidence_bundle(
        &bundle("dependent-bundle", vec![derived])?,
        &authorization()?,
    )?;
    assert!(matches!(
        repository.rollback_evidence_bundle("source-bundle"),
        Err(RepositoryError::EvidenceRollbackBlocked { referenced_by, .. })
            if referenced_by.contains("derived-candidate")
    ));
    assert!(repository.get_evidence_item("raw-observation")?.is_some());
    assert_eq!(repository.list_nodes()?.len(), 2);
    Ok(())
}

#[test]
fn populated_plain_database_remains_deterministic_through_apply_and_rollback() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("populated.memory3d"))?;
    let plain = repository.add_node(NewMemory::new(
        MemoryKind::new("plain-note")?,
        "Plain token refresh memory",
        0.8,
    )?)?;
    let options = ActivationOptions {
        include_seeds: true,
        ..ActivationOptions::default()
    };
    let before = repository.activate("plain token refresh", &options)?;
    assert_eq!(before.results[0].node.id(), plain.id());

    let applied_bundle = bundle(
        "populated-bundle",
        vec![item(
            "populated-evidence",
            "Filesystem checkpoints use BYTE_OFFSET.",
            "filesystem",
            "observed",
        )],
    )?;
    repository.apply_evidence_bundle(&applied_bundle, &authorization()?)?;
    let after_apply = repository.activate("plain token refresh", &options)?;
    assert_eq!(after_apply.results, before.results);
    assert_eq!(repository.get_node(plain.id())?, Some(plain.clone()));

    let duplicate = bundle(
        "different-key-same-id",
        vec![item(
            "populated-evidence",
            "Attempted overwrite",
            "filesystem",
            "candidate",
        )],
    )?;
    assert!(matches!(
        repository.preview_evidence_bundle(&duplicate),
        Err(RepositoryError::EvidenceIdExists(id)) if id == "populated-evidence"
    ));

    repository.rollback_evidence_bundle("populated-bundle")?;
    let after_rollback = repository.activate("plain token refresh", &options)?;
    assert_eq!(after_rollback.results, before.results);
    assert_eq!(repository.list_nodes()?, vec![plain]);
    Ok(())
}
