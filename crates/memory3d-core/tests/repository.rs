//! Repository acceptance tests for Task 02 durability invariants.

use std::{
    collections::hash_map::DefaultHasher,
    error::Error,
    fs,
    hash::{Hash, Hasher},
};

use memory3d_core::{
    Coordinates, MemoryKind, Metadata, NewMemory, NewRelation, NodeId, Repository, RepositoryError,
};
use rusqlite::{Connection, params};
use serde_json::json;
use tempfile::tempdir;

type TestResult = Result<(), Box<dyn Error>>;

fn memory(text: &str) -> Result<NewMemory, Box<dyn Error>> {
    Ok(NewMemory::new(MemoryKind::new("decision")?, text, 0.75)?)
}

#[test]
fn creates_empty_versioned_database() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("empty.memory3d");
    let repository = Repository::open(&path)?;

    assert!(path.is_file());
    assert!(repository.list_nodes()?.is_empty());
    assert!(repository.list_relations()?.is_empty());
    drop(repository);

    let connection = Connection::open(path)?;
    let metadata: (String, i64) = connection.query_row(
        "SELECT format_id, schema_version FROM memory3d_schema WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(metadata, ("memory3d".to_owned(), 7));
    Ok(())
}

#[test]
fn node_is_identical_after_reopen_and_append_preserves_it() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("reopen.memory3d");
    let metadata = Metadata::new(json!({
        "owner": "AuthService",
        "flags": ["mobile", "refresh"],
        "nested": {"active": true}
    }))?;
    let coordinates = Coordinates::new(-1.25, 0.0, 42.5)?;
    let original = {
        let mut repository = Repository::open(&path)?;
        repository.add_node(
            memory("Token refresh owns session continuity")?
                .with_metadata(metadata)
                .with_coordinates(coordinates),
        )?
    };

    let mut reopened = Repository::open(&path)?;
    assert_eq!(reopened.get_node(original.id())?, Some(original.clone()));

    let appended = reopened.add_node(memory("A later independent memory")?)?;
    assert!(appended.id() > original.id());
    assert_eq!(reopened.get_node(original.id())?, Some(original.clone()));
    drop(reopened);

    let reopened_again = Repository::open(path)?;
    assert_eq!(reopened_again.list_nodes()?, vec![original, appended]);
    Ok(())
}

#[test]
fn relations_require_existing_nodes_and_round_trip_updates() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("relations.memory3d");
    let mut repository = Repository::open(&path)?;
    let source = repository.add_node(memory("AuthService")?)?;
    let target = repository.add_node(memory("TokenRefresh")?)?;
    let missing = NodeId::new(target.id().get() + 1)?;

    let error = repository.add_relation(NewRelation::new(source.id(), missing, "owns", 0.9)?);
    assert!(matches!(error, Err(RepositoryError::NodeNotFound(id)) if id == missing));
    assert!(repository.list_relations()?.is_empty());

    let relation =
        repository.add_relation(NewRelation::new(source.id(), target.id(), "owns", 0.9)?)?;
    assert_eq!(
        repository.get_relation(source.id(), target.id(), "owns")?,
        Some(relation)
    );

    let updated = repository.update_relation_weight(source.id(), target.id(), "owns", 0.4)?;
    assert!((updated.weight() - 0.4).abs() < f32::EPSILON);
    assert!(updated.updated_at_ms() >= updated.created_at_ms());
    drop(repository);

    let reopened = Repository::open(path)?;
    assert_eq!(
        reopened.get_relation(source.id(), target.id(), "owns")?,
        Some(updated)
    );
    Ok(())
}

#[test]
fn relation_batch_rolls_back_on_conflict() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("transaction.memory3d");
    let mut repository = Repository::open(&path)?;
    let source = repository.add_node(memory("source")?)?;
    let target = repository.add_node(memory("target")?)?;
    let other = repository.add_node(memory("other target")?)?;
    let committed =
        repository.add_relation(NewRelation::new(source.id(), target.id(), "owns", 0.9)?)?;
    let duplicate = NewRelation::new(source.id(), other.id(), "depends_on", 0.8)?;

    assert!(
        repository
            .add_relations(&[duplicate.clone(), duplicate])
            .is_err()
    );
    assert_eq!(repository.list_relations()?, vec![committed.clone()]);
    drop(repository);
    let reopened = Repository::open(path)?;
    assert_eq!(reopened.list_relations()?, vec![committed]);
    Ok(())
}

#[test]
fn node_batch_rolls_back_and_preserves_committed_data() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("node-rollback.memory3d");
    let committed = {
        let mut repository = Repository::open(&path)?;
        repository.add_node(memory("committed before failed batch")?)?
    };
    let connection = Connection::open(&path)?;
    connection.execute_batch(
        "CREATE TRIGGER reject_test_node BEFORE INSERT ON nodes WHEN NEW.text = 'reject me' BEGIN SELECT RAISE(ABORT, 'injected test failure'); END;",
    )?;
    drop(connection);

    let mut repository = Repository::open(&path)?;
    let failed = repository.add_nodes(&[memory("first uncommitted node")?, memory("reject me")?]);
    assert!(failed.is_err());
    assert_eq!(repository.list_nodes()?, vec![committed.clone()]);
    drop(repository);

    let reopened = Repository::open(path)?;
    assert_eq!(reopened.list_nodes()?, vec![committed]);
    Ok(())
}

#[test]
fn competing_writer_gets_typed_busy_error_and_committed_data_survives() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("busy.memory3d");
    let committed = {
        let mut repository = Repository::open(&path)?;
        repository.add_node(memory("committed before lock")?)?
    };
    let blocker = Connection::open(&path)?;
    blocker.execute_batch("BEGIN IMMEDIATE")?;

    let mut competing = Repository::open(&path)?;
    let error = competing.add_node(memory("blocked write")?);
    assert!(matches!(&error, Err(RepositoryError::Busy)));
    assert!(
        error
            .err()
            .is_some_and(|value| value.to_string().contains("busy or locked"))
    );
    drop(competing);
    blocker.execute_batch("ROLLBACK")?;
    drop(blocker);

    let reopened = Repository::open(path)?;
    assert_eq!(reopened.list_nodes()?, vec![committed]);
    Ok(())
}

#[test]
fn integrity_check_is_read_only_and_accepts_valid_database() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("integrity.memory3d");
    let mut repository = Repository::open(&path)?;
    repository.add_node(memory("durable integrity fixture")?)?;
    drop(repository);

    let before = fs::read(&path)?;
    let report = Repository::check(&path)?;
    assert!(report.is_ok());
    assert!(report.messages().is_empty());
    assert_eq!(fs::read(path)?, before);
    Ok(())
}

#[test]
fn schema_constraints_reject_raw_invalid_values() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("constraints.memory3d");
    drop(Repository::open(&path)?);
    let connection = Connection::open(path)?;
    connection.pragma_update(None, "foreign_keys", true)?;

    let invalid_node = connection.execute(
        "INSERT INTO nodes (kind, text, metadata, importance, created_at_ms, updated_at_ms) VALUES ('fact', 'text', '[]', 2.0, 1, 1)",
        [],
    );
    assert!(invalid_node.is_err());

    let missing_relation = connection.execute(
        "INSERT INTO relations (source_id, target_id, name, weight, created_at_ms, updated_at_ms) VALUES (1, 2, 'owns', 0.5, 1, 1)",
        [],
    );
    assert!(missing_relation.is_err());
    Ok(())
}

#[test]
fn migrates_version_zero_transactionally() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("migration.memory3d");
    let connection = Connection::open(&path)?;
    connection.execute_batch(
        "CREATE TABLE memory3d_schema (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), format_id TEXT NOT NULL, schema_version INTEGER NOT NULL CHECK (schema_version >= 0), created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0));",
    )?;
    connection.execute(
        "INSERT INTO memory3d_schema VALUES (1, 'memory3d', 0, 1)",
        [],
    )?;
    drop(connection);

    let repository = Repository::open(&path)?;
    assert!(repository.list_nodes()?.is_empty());
    drop(repository);

    let connection = Connection::open(path)?;
    let version: i64 = connection.query_row(
        "SELECT schema_version FROM memory3d_schema WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(version, 7);
    Ok(())
}

#[test]
fn migrates_version_two_with_adjacency_index() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("adjacency-migration.memory3d");
    drop(Repository::open(&path)?);
    let connection = Connection::open(&path)?;
    connection.execute_batch(
        "DROP INDEX evidence_decision_support_target_idx;
         DROP TABLE evidence_decision_support;
         DROP INDEX evidence_supersessions_target_idx;
         DROP TABLE evidence_supersessions;
         DROP INDEX evidence_conflicts_target_idx;
         DROP TABLE evidence_conflicts;
         DROP INDEX evidence_derivations_support_idx;
         DROP TABLE evidence_derivations;
         DROP INDEX evidence_items_conflict_idx;
         DROP INDEX evidence_items_node_idx;
         DROP INDEX evidence_items_bundle_idx;
         DROP TABLE evidence_items;
         DROP TABLE evidence_bundles;
         DROP TRIGGER community_nodes_insert_revision;
         DROP TRIGGER community_nodes_update_revision;
         DROP TRIGGER community_nodes_delete_revision;
         DROP TRIGGER community_relations_insert_revision;
         DROP TRIGGER community_relations_update_revision;
         DROP TRIGGER community_relations_delete_revision;
         DROP INDEX community_memberships_lookup_idx;
         DROP TABLE community_memberships;
         DROP TABLE community_builds;
         DROP TABLE community_graph_state;
         DROP INDEX relation_feedback_relation_idx;
         DROP TABLE relation_feedback_events;
         DROP INDEX relations_adjacency_idx;
         UPDATE memory3d_schema SET schema_version = 2 WHERE singleton = 1;",
    )?;
    drop(connection);

    drop(Repository::open(&path)?);
    let connection = Connection::open(path)?;
    let version: i64 = connection.query_row(
        "SELECT schema_version FROM memory3d_schema WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    let index_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'relations_adjacency_idx')",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(version, 7);
    assert!(index_exists);
    Ok(())
}

#[test]
fn migrates_previous_version_four_with_community_schema() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("community-migration.memory3d");
    drop(Repository::open(&path)?);
    let connection = Connection::open(&path)?;
    connection.execute_batch(
        "DROP INDEX evidence_decision_support_target_idx;
         DROP TABLE evidence_decision_support;
         DROP INDEX evidence_supersessions_target_idx;
         DROP TABLE evidence_supersessions;
         DROP INDEX evidence_conflicts_target_idx;
         DROP TABLE evidence_conflicts;
         DROP INDEX evidence_derivations_support_idx;
         DROP TABLE evidence_derivations;
         DROP INDEX evidence_items_conflict_idx;
         DROP INDEX evidence_items_node_idx;
         DROP INDEX evidence_items_bundle_idx;
         DROP TABLE evidence_items;
         DROP TABLE evidence_bundles;
         DROP TRIGGER community_nodes_insert_revision;
         DROP TRIGGER community_nodes_update_revision;
         DROP TRIGGER community_nodes_delete_revision;
         DROP TRIGGER community_relations_insert_revision;
         DROP TRIGGER community_relations_update_revision;
         DROP TRIGGER community_relations_delete_revision;
         DROP INDEX community_memberships_lookup_idx;
         DROP TABLE community_memberships;
         DROP TABLE community_builds;
         DROP TABLE community_graph_state;
         UPDATE memory3d_schema SET schema_version = 4 WHERE singleton = 1;",
    )?;
    drop(connection);

    drop(Repository::open(&path)?);
    let connection = Connection::open(path)?;
    let version: i64 = connection.query_row(
        "SELECT schema_version FROM memory3d_schema WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    let state_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'community_graph_state')",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(version, 7);
    assert!(state_exists);
    Ok(())
}

#[test]
fn newer_schema_is_rejected_without_changing_file_hash() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("future.memory3d");
    let committed = {
        let mut repository = Repository::open(&path)?;
        repository.add_node(memory("committed before future schema marker")?)?
    };
    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE memory3d_schema SET schema_version = ?1 WHERE singleton = 1",
        params![8],
    )?;
    drop(connection);

    let before = fs::read(&path)?;
    let before_hash = hash(&before);
    let opened = Repository::open(&path);
    assert!(matches!(
        opened,
        Err(RepositoryError::UnsupportedSchema {
            found: 8,
            supported: 7
        })
    ));
    let after = fs::read(&path)?;
    assert_eq!(hash(&after), before_hash);
    assert_eq!(after, before);
    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE memory3d_schema SET schema_version = 7 WHERE singleton = 1",
        [],
    )?;
    drop(connection);
    let reopened = Repository::open(path)?;
    assert_eq!(reopened.list_nodes()?, vec![committed]);
    Ok(())
}

fn hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}
