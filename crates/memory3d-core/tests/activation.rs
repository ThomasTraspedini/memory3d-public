//! Task 03 acceptance tests for lexical search and explainable bounded activation.

use std::error::Error;

use memory3d_core::{
    ActivationOptions, MemoryKind, NewMemory, NewRelation, NodeId, Repository, SearchOptions,
    ValidationError,
};
use rusqlite::Connection;
use tempfile::tempdir;

type TestResult = Result<(), Box<dyn Error>>;

fn memory(kind: &str, text: &str, importance: f32) -> Result<NewMemory, ValidationError> {
    NewMemory::new(MemoryKind::new(kind)?, text, importance)
}

#[test]
fn lexical_search_normalizes_ranks_filters_and_handles_no_match() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("search.memory3d"))?;
    let exact = repository.add_node(memory(
        "component",
        "TOKEN-refresh keeps mobile sessions alive",
        0.1,
    )?)?;
    let partial = repository.add_node(memory("incident", "Refresh jobs were delayed", 1.0)?)?;
    repository.add_node(memory("component", "Checkout tax calculation", 1.0)?)?;

    let results = repository.search("Token, REFRESH token!", &SearchOptions::default())?;
    assert_eq!(
        results
            .iter()
            .map(|result| result.node.id())
            .collect::<Vec<_>>(),
        vec![exact.id(), partial.id()]
    );
    assert_eq!(results[0].matched_terms, 2);
    assert!((results[0].score - 1.0).abs() < f32::EPSILON);
    assert!((results[1].score - 0.5).abs() < f32::EPSILON);

    let filtered = repository.search(
        "refresh",
        &SearchOptions {
            kind: Some(MemoryKind::new("incident")?),
            ..SearchOptions::default()
        },
    )?;
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].node.id(), partial.id());
    assert!(
        repository
            .search("unfindable", &SearchOptions::default())?
            .is_empty()
    );
    let no_match = repository.activate("unfindable", &ActivationOptions::default())?;
    assert!(no_match.results.is_empty());
    assert_eq!(no_match.stats.visited_nodes, 0);
    assert_eq!(no_match.stats.visited_edges, 0);
    Ok(())
}

#[test]
fn activation_returns_direct_and_indirect_paths_with_formula_decay() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("paths.memory3d"))?;
    let seed = repository.add_node(memory("component", "Token refresh implementation", 1.0)?)?;
    let direct = repository.add_node(memory("dependency", "Authentication gateway", 1.0)?)?;
    let indirect = repository.add_node(memory(
        "incident",
        "Mobile sessions were lost after deployment",
        0.5,
    )?)?;
    repository.add_node(memory(
        "distractor",
        "Refresh the token color palette",
        1.0,
    )?)?;
    repository.add_relation(NewRelation::new(seed.id(), direct.id(), "owns", 0.5)?)?;
    repository.add_relation(NewRelation::new(direct.id(), indirect.id(), "caused", 0.5)?)?;

    let report = repository.activate(
        "token refresh implementation",
        &ActivationOptions {
            seed_limit: 1,
            ..ActivationOptions::default()
        },
    )?;
    let direct_result = report
        .results
        .iter()
        .find(|result| result.node.id() == direct.id())
        .ok_or("missing direct result")?;
    let indirect_result = report
        .results
        .iter()
        .find(|result| result.node.id() == indirect.id())
        .ok_or("missing indirect result")?;
    assert_eq!(direct_result.hops(), 1);
    assert_eq!(indirect_result.hops(), 2);
    assert_eq!(indirect_result.path.seed, seed.id());
    assert_eq!(
        indirect_result
            .path
            .steps
            .iter()
            .map(|step| step.relation.as_str())
            .collect::<Vec<_>>(),
        vec!["owns", "caused"]
    );
    assert!((direct_result.score - (1.0 * 0.5 * 0.8 * 1.0)).abs() < f32::EPSILON);
    assert!((indirect_result.score - (1.0 * 0.5 * 0.5 * 0.8 * 0.8 * 0.75)).abs() < f32::EPSILON);
    assert!(direct_result.score > indirect_result.score);
    for result in &report.results {
        for step in &result.path.steps {
            assert!(
                repository
                    .get_relation(step.source, step.target, &step.relation)?
                    .is_some()
            );
        }
    }
    Ok(())
}

#[test]
fn best_path_wins_cycles_terminate_and_ties_use_node_id() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("choices.memory3d"))?;
    let seed = repository.add_node(memory("component", "Token refresh", 1.0)?)?;
    let low = repository.add_node(memory("fact", "low route", 1.0)?)?;
    let high = repository.add_node(memory("fact", "high route", 1.0)?)?;
    let target = repository.add_node(memory("incident", "session loss", 1.0)?)?;
    let tie_first = repository.add_node(memory("risk", "first tie", 1.0)?)?;
    let tie_second = repository.add_node(memory("risk", "second tie", 1.0)?)?;
    repository.add_relations(&[
        NewRelation::new(seed.id(), low.id(), "low", 0.4)?,
        NewRelation::new(seed.id(), high.id(), "high", 0.9)?,
        NewRelation::new(low.id(), target.id(), "reaches", 1.0)?,
        NewRelation::new(high.id(), target.id(), "reaches", 1.0)?,
        NewRelation::new(target.id(), seed.id(), "cycle", 1.0)?,
        NewRelation::new(seed.id(), tie_first.id(), "tie", 0.5)?,
        NewRelation::new(seed.id(), tie_second.id(), "tie", 0.5)?,
    ])?;

    let report = repository.activate(
        "token refresh",
        &ActivationOptions {
            seed_limit: 1,
            ..ActivationOptions::default()
        },
    )?;
    let target_result = report
        .results
        .iter()
        .find(|result| result.node.id() == target.id())
        .ok_or("missing multi-path target")?;
    assert_eq!(target_result.path.steps[0].target, high.id());
    let first_position = report
        .results
        .iter()
        .position(|result| result.node.id() == tie_first.id())
        .ok_or("missing first tie")?;
    let second_position = report
        .results
        .iter()
        .position(|result| result.node.id() == tie_second.id())
        .ok_or("missing second tie")?;
    assert!(first_position < second_position);
    assert!(report.results.iter().all(|result| result.hops() <= 3));
    assert!(report.stats.visited_edges <= 5_000);
    Ok(())
}

#[test]
fn seed_filter_hop_limit_and_work_budgets_are_enforced() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("bounds.memory3d"))?;
    let seed = repository.add_node(memory("component", "Token refresh", 1.0)?)?;
    let first = repository.add_node(memory("fact", "first", 1.0)?)?;
    let second = repository.add_node(memory("fact", "second", 1.0)?)?;
    repository.add_relations(&[
        NewRelation::new(seed.id(), first.id(), "next", 1.0)?,
        NewRelation::new(first.id(), second.id(), "next", 1.0)?,
    ])?;

    let no_seeds = repository.activate(
        "token refresh",
        &ActivationOptions {
            hops: 1,
            seed_limit: 1,
            max_visited_nodes: 2,
            max_visited_edges: 1,
            ..ActivationOptions::default()
        },
    )?;
    assert_eq!(no_seeds.results.len(), 1);
    assert_eq!(no_seeds.results[0].node.id(), first.id());
    assert_eq!(no_seeds.stats.visited_nodes, 2);
    assert_eq!(no_seeds.stats.visited_edges, 1);
    assert_eq!(no_seeds.database.seed_queries, 1);
    assert_eq!(no_seeds.database.seed_node_reads, 1);
    assert_eq!(no_seeds.database.adjacency_queries, 1);
    assert_eq!(no_seeds.database.traversal_node_reads, 1);

    let with_seed = repository.activate(
        "token refresh",
        &ActivationOptions {
            hops: 0,
            seed_limit: 1,
            include_seeds: true,
            ..ActivationOptions::default()
        },
    )?;
    assert_eq!(with_seed.results.len(), 1);
    assert_eq!(with_seed.results[0].node.id(), seed.id());
    assert_eq!(with_seed.results[0].hops(), 0);
    assert_eq!(with_seed.stats.visited_edges, 0);
    Ok(())
}

#[test]
fn every_activation_budget_is_hard_and_reported() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("hard-bounds.memory3d"))?;
    let first_seed = repository.add_node(memory("component", "Orion catalyst alpha", 1.0)?)?;
    let second_seed = repository.add_node(memory("component", "Orion catalyst beta", 1.0)?)?;
    let targets = repository.add_nodes(
        &(0..8)
            .map(|index| memory("fact", &format!("target {index}"), 1.0))
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let mut relations = Vec::new();
    for target in &targets[..4] {
        relations.push(NewRelation::new(
            first_seed.id(),
            target.id(),
            "reaches",
            1.0,
        )?);
    }
    for target in &targets[4..] {
        relations.push(NewRelation::new(
            second_seed.id(),
            target.id(),
            "reaches",
            1.0,
        )?);
    }
    repository.add_relations(&relations)?;

    let report = repository.activate(
        "orion catalyst",
        &ActivationOptions {
            hops: 1,
            seed_limit: 1,
            limit: 2,
            max_visited_nodes: 3,
            max_visited_edges: 2,
            include_seeds: false,
        },
    )?;
    assert_eq!(report.stats.seeds, 1);
    assert_eq!(report.stats.visited_nodes, 3);
    assert_eq!(report.stats.visited_edges, 2);
    assert_eq!(report.results.len(), 2);
    assert!(report.results.iter().all(|result| result.hops() <= 1));
    assert_eq!(report.database.seed_queries, 1);
    assert_eq!(report.database.seed_node_reads, 1);
    assert_eq!(report.database.adjacency_queries, 1);
    assert_eq!(report.database.traversal_node_reads, 2);
    Ok(())
}

#[test]
fn candidate_selection_prefers_evidence_over_earlier_low_value_noise() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("candidate-order.memory3d"))?;
    let noise_seed =
        repository.add_node(memory("archive", "Orion catalyst lexical collision", 0.1)?)?;
    let seed = repository.add_node(memory("component", "Orion catalyst change point", 1.0)?)?;
    let noise = repository.add_node(memory("archive", "connected low value noise", 0.1)?)?;
    let evidence = repository.add_node(memory("evidence", "deployment risk", 1.0)?)?;
    repository.add_relations(&[
        NewRelation::new(seed.id(), noise.id(), "mentions", 0.1)?,
        NewRelation::new(seed.id(), evidence.id(), "reveals", 1.0)?,
    ])?;

    let report = repository.activate(
        "orion catalyst",
        &ActivationOptions {
            hops: 1,
            seed_limit: 1,
            limit: 1,
            max_visited_nodes: 2,
            max_visited_edges: 1,
            include_seeds: false,
        },
    )?;

    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].path.seed, seed.id());
    assert_eq!(report.results[0].node.id(), evidence.id());
    assert_ne!(report.results[0].path.seed, noise_seed.id());
    assert_eq!(report.stats.visited_edges, 1);
    Ok(())
}

#[test]
fn stronger_edge_enters_a_tight_budget_before_earlier_noise() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("edge-order.memory3d"))?;
    let seed = repository.add_node(memory("component", "Orion catalyst change point", 1.0)?)?;
    let noise = repository.add_node(memory("archive", "connected low value noise", 0.1)?)?;
    let evidence = repository.add_node(memory("evidence", "deployment risk", 1.0)?)?;
    repository.add_relations(&[
        NewRelation::new(seed.id(), noise.id(), "mentions", 0.1)?,
        NewRelation::new(seed.id(), evidence.id(), "reveals", 1.0)?,
    ])?;

    let report = repository.activate(
        "orion catalyst",
        &ActivationOptions {
            hops: 1,
            seed_limit: 1,
            limit: 1,
            max_visited_nodes: 2,
            max_visited_edges: 1,
            include_seeds: false,
        },
    )?;
    assert_eq!(report.results[0].node.id(), evidence.id());
    assert_eq!(report.results[0].path.steps[0].relation, "reveals");
    Ok(())
}

#[test]
fn invalid_queries_and_options_fail_before_traversal() -> TestResult {
    let directory = tempdir()?;
    let repository = Repository::open(directory.path().join("invalid.memory3d"))?;
    assert!(repository.search("---", &SearchOptions::default()).is_err());
    assert!(
        repository
            .search(
                "query",
                &SearchOptions {
                    limit: 0,
                    kind: None,
                },
            )
            .is_err()
    );
    assert!(
        repository
            .activate(
                "query",
                &ActivationOptions {
                    seed_limit: 0,
                    ..ActivationOptions::default()
                },
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn migrated_index_and_activation_are_identical_after_reopen() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("reopen.memory3d");
    let (seed_id, target_id, before) = {
        let mut repository = Repository::open(&path)?;
        let seed = repository.add_node(memory("component", "Token refresh", 1.0)?)?;
        let target = repository.add_node(memory("incident", "Mobile session loss", 1.0)?)?;
        repository.add_relation(NewRelation::new(seed.id(), target.id(), "caused", 0.75)?)?;
        let report = repository.activate(
            "token refresh",
            &ActivationOptions {
                seed_limit: 1,
                ..ActivationOptions::default()
            },
        )?;
        (seed.id(), target.id(), report)
    };

    let reopened = Repository::open(&path)?;
    let after = reopened.activate(
        "token refresh",
        &ActivationOptions {
            seed_limit: 1,
            ..ActivationOptions::default()
        },
    )?;
    assert_eq!(before.results, after.results);
    assert_eq!(before.stats, after.stats);
    assert_eq!(before.database, after.database);
    let step = &after.results[0].path.steps[0];
    assert_eq!((step.source, step.target), (seed_id, target_id));
    assert!(
        reopened
            .get_relation(step.source, step.target, &step.relation)?
            .is_some()
    );
    drop(reopened);

    let connection = Connection::open(&path)?;
    connection.execute_batch(
        "DROP INDEX relations_adjacency_idx; DROP INDEX node_terms_term_idx; DROP TABLE node_terms; UPDATE memory3d_schema SET schema_version = 1 WHERE singleton = 1;",
    )?;
    drop(connection);
    let migrated = Repository::open(path)?;
    let search = migrated.search("TOKEN refresh", &SearchOptions::default())?;
    assert_eq!(search[0].node.id(), seed_id);
    Ok(())
}

#[allow(dead_code)]
fn _assert_node_id_is_public(_: NodeId) {}
