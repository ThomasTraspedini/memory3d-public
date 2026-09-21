//! Task 13 explicit feedback and deterministic decay regression tests.

use std::error::Error;

use memory3d_core::{
    ActivationOptions, FeedbackKind, FeedbackPolicy, MemoryKind, NewFeedbackEvent, NewMemory,
    NewRelation, Repository,
};
use tempfile::tempdir;

type TestResult = Result<(), Box<dyn Error>>;

fn memory(kind: &str, text: &str, importance: f32) -> Result<NewMemory, Box<dyn Error>> {
    Ok(NewMemory::new(MemoryKind::new(kind)?, text, importance)?)
}

fn tight_options() -> ActivationOptions {
    ActivationOptions {
        hops: 1,
        seed_limit: 1,
        limit: 1,
        max_visited_nodes: 2,
        max_visited_edges: 1,
        include_seeds: false,
    }
}

#[test]
fn repeated_positive_feedback_changes_tight_budget_order_without_weight_mutation() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("positive.memory3d"))?;
    let seed = repository.add_node(memory("component", "Feedback probe", 1.0)?)?;
    let baseline_winner =
        repository.add_node(memory("evidence", "Base stronger evidence", 1.0)?)?;
    let reinforced =
        repository.add_node(memory("evidence", "Operator reinforced evidence", 1.0)?)?;
    repository.add_relation(NewRelation::new(
        seed.id(),
        baseline_winner.id(),
        "reveals",
        0.6,
    )?)?;
    repository.add_relation(NewRelation::new(
        seed.id(),
        reinforced.id(),
        "reveals",
        0.45,
    )?)?;

    let plain = repository.activate("feedback probe", &tight_options())?;
    assert_eq!(plain.results[0].node.id(), baseline_winner.id());

    for occurred_at_ms in [1_000, 2_000] {
        repository.record_relation_feedback(NewFeedbackEvent::new(
            seed.id(),
            reinforced.id(),
            "reveals",
            FeedbackKind::Positive,
            1.0,
            occurred_at_ms,
        )?)?;
    }
    let feedback = repository.activate_with_feedback(
        "feedback probe",
        &tight_options(),
        &FeedbackPolicy::explicit_v1(),
    )?;
    assert_eq!(feedback.results[0].node.id(), reinforced.id());
    assert!(feedback.results[0].score.is_finite());
    let stored_weight = repository
        .get_relation(seed.id(), reinforced.id(), "reveals")?
        .ok_or("missing relation")?
        .weight();
    assert!((stored_weight - 0.45).abs() < f32::EPSILON);
    assert_eq!(
        repository
            .get_relation(seed.id(), reinforced.id(), "reveals")?
            .ok_or("missing relation")?
            .reinforcement_count(),
        2
    );
    Ok(())
}

#[test]
fn negative_feedback_can_demote_a_relation_under_tight_budget() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("negative.memory3d"))?;
    let seed = repository.add_node(memory("component", "Feedback probe", 1.0)?)?;
    let demoted = repository.add_node(memory("evidence", "Demoted evidence", 1.0)?)?;
    let fallback = repository.add_node(memory("evidence", "Fallback evidence", 1.0)?)?;
    repository.add_relation(NewRelation::new(seed.id(), demoted.id(), "reveals", 0.6)?)?;
    repository.add_relation(NewRelation::new(seed.id(), fallback.id(), "reveals", 0.45)?)?;

    let plain = repository.activate("feedback probe", &tight_options())?;
    assert_eq!(plain.results[0].node.id(), demoted.id());
    repository.record_relation_feedback(NewFeedbackEvent::new(
        seed.id(),
        demoted.id(),
        "reveals",
        FeedbackKind::Negative,
        1.0,
        3_000,
    )?)?;

    let feedback = repository.activate_with_feedback(
        "feedback probe",
        &tight_options(),
        &FeedbackPolicy::explicit_v1(),
    )?;
    assert_eq!(feedback.results[0].node.id(), fallback.id());
    assert_eq!(
        repository
            .get_relation(seed.id(), demoted.id(), "reveals")?
            .ok_or("missing relation")?
            .reinforcement_count(),
        0
    );
    Ok(())
}

#[test]
fn stale_feedback_decays_only_from_injected_time() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("stale.memory3d"))?;
    let seed = repository.add_node(memory("component", "Feedback probe", 1.0)?)?;
    let fresh = repository.add_node(memory("evidence", "Fresh operator evidence", 1.0)?)?;
    let old = repository.add_node(memory("evidence", "Stale operator evidence", 1.0)?)?;
    repository.add_relation(NewRelation::new(seed.id(), fresh.id(), "reveals", 0.45)?)?;
    repository.add_relation(NewRelation::new(seed.id(), old.id(), "reveals", 0.45)?)?;
    repository.record_relation_feedback(NewFeedbackEvent::new(
        seed.id(),
        old.id(),
        "reveals",
        FeedbackKind::Positive,
        1.0,
        0,
    )?)?;
    repository.record_relation_feedback(NewFeedbackEvent::new(
        seed.id(),
        fresh.id(),
        "reveals",
        FeedbackKind::Positive,
        1.0,
        9_000,
    )?)?;

    let report = repository.activate_with_feedback(
        "feedback probe",
        &tight_options(),
        &FeedbackPolicy::explicit_v1().with_decay(9_000, 1_000),
    )?;
    assert_eq!(report.results[0].node.id(), fresh.id());
    Ok(())
}

#[test]
fn feedback_survives_reopen_and_rollback_restores_feedback_aware_order() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("reopen.memory3d");
    let (seed, baseline, boosted, event_id, before) = {
        let mut repository = Repository::open(&path)?;
        let seed = repository.add_node(memory("component", "Feedback probe", 1.0)?)?;
        let baseline = repository.add_node(memory("evidence", "Baseline evidence", 1.0)?)?;
        let boosted = repository.add_node(memory("evidence", "Boosted evidence", 1.0)?)?;
        repository.add_relation(NewRelation::new(seed.id(), baseline.id(), "reveals", 0.6)?)?;
        repository.add_relation(NewRelation::new(seed.id(), boosted.id(), "reveals", 0.5)?)?;
        let event = repository.record_relation_feedback(NewFeedbackEvent::new(
            seed.id(),
            boosted.id(),
            "reveals",
            FeedbackKind::Positive,
            1.0,
            1_000,
        )?)?;
        let before = repository.activate_with_feedback(
            "feedback probe",
            &tight_options(),
            &FeedbackPolicy::explicit_v1(),
        )?;
        (seed, baseline, boosted, event.id(), before)
    };

    let mut reopened = Repository::open(&path)?;
    let after = reopened.activate_with_feedback(
        "feedback probe",
        &tight_options(),
        &FeedbackPolicy::explicit_v1(),
    )?;
    assert_eq!(after.results, before.results);
    assert_eq!(after.results[0].node.id(), boosted.id());
    assert_eq!(
        reopened.list_relation_feedback(seed.id(), boosted.id(), "reveals")?[0].id(),
        event_id
    );

    let removed = reopened.remove_feedback_event(event_id)?;
    assert_eq!(removed.id(), event_id);
    let rolled_back = reopened.activate_with_feedback(
        "feedback probe",
        &tight_options(),
        &FeedbackPolicy::explicit_v1(),
    )?;
    assert_eq!(rolled_back.results[0].node.id(), baseline.id());
    assert!(
        reopened
            .list_relation_feedback(seed.id(), boosted.id(), "reveals")?
            .is_empty()
    );
    Ok(())
}

#[test]
fn equal_feedback_uses_stable_target_tie_break() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("ties.memory3d"))?;
    let seed = repository.add_node(memory("component", "Feedback probe", 1.0)?)?;
    let first = repository.add_node(memory("evidence", "First equal evidence", 1.0)?)?;
    let second = repository.add_node(memory("evidence", "Second equal evidence", 1.0)?)?;
    repository.add_relation(NewRelation::new(seed.id(), first.id(), "reveals", 0.5)?)?;
    repository.add_relation(NewRelation::new(seed.id(), second.id(), "reveals", 0.5)?)?;
    for target in [first.id(), second.id()] {
        repository.record_relation_feedback(NewFeedbackEvent::new(
            seed.id(),
            target,
            "reveals",
            FeedbackKind::Positive,
            1.0,
            1_000,
        )?)?;
    }

    let report = repository.activate_with_feedback(
        "feedback probe",
        &tight_options(),
        &FeedbackPolicy::explicit_v1(),
    )?;
    assert_eq!(report.results[0].node.id(), first.id());
    Ok(())
}
