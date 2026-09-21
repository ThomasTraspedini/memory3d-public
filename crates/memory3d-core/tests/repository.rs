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
    assert_eq!(metadata, ("memory3d".to_owned(), 1));
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
    let mut repository = Repository::open(path)?;
    let source = repository.add_node(memory("source")?)?;
    let target = repository.add_node(memory("target")?)?;
    let duplicate = NewRelation::new(source.id(), target.id(), "depends_on", 0.8)?;

    assert!(
        repository
            .add_relations(&[duplicate.clone(), duplicate])
            .is_err()
    );
    assert!(repository.list_relations()?.is_empty());
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
    assert_eq!(version, 1);
    Ok(())
}

#[test]
fn newer_schema_is_rejected_without_changing_file_hash() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("future.memory3d");
    drop(Repository::open(&path)?);
    let connection = Connection::open(&path)?;
    connection.execute(
        "UPDATE memory3d_schema SET schema_version = ?1 WHERE singleton = 1",
        params![2],
    )?;
    drop(connection);

    let before = fs::read(&path)?;
    let before_hash = hash(&before);
    let opened = Repository::open(&path);
    assert!(matches!(
        opened,
        Err(RepositoryError::UnsupportedSchema {
            found: 2,
            supported: 1
        })
    ));
    let after = fs::read(path)?;
    assert_eq!(hash(&after), before_hash);
    assert_eq!(after, before);
    Ok(())
}

fn hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}
