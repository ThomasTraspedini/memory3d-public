//! Public input-limit regression tests for persistence and retrieval boundaries.

use std::error::Error;

use memory3d_core::{
    ActivationOptions, MAX_HOPS, MAX_METADATA_BYTES, MAX_QUERY_BYTES, MAX_RELATION_NAME_BYTES,
    MAX_RESULTS, MAX_SEEDS, MAX_TEXT_BYTES, MemoryKind, Metadata, NewMemory, NewRelation, NodeId,
    Repository, SearchOptions, ValidationError,
};
use serde_json::json;
use tempfile::tempdir;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn persistence_payload_limits_accept_caps_and_reject_larger_values() -> TestResult {
    let kind = MemoryKind::new("fact")?;
    assert!(NewMemory::new(kind.clone(), "x".repeat(MAX_TEXT_BYTES), 0.5).is_ok());
    assert!(matches!(
        NewMemory::new(kind, "x".repeat(MAX_TEXT_BYTES + 1), 0.5),
        Err(ValidationError::TooLong {
            field: "text",
            max_bytes: MAX_TEXT_BYTES
        })
    ));
    assert!(
        Metadata::new(json!({
            "value": "x".repeat(MAX_METADATA_BYTES - br#"{"value":""}"#.len())
        }))
        .is_ok()
    );
    assert!(matches!(
        Metadata::new(json!({"value": "x".repeat(MAX_METADATA_BYTES)})),
        Err(ValidationError::TooLong {
            field: "metadata",
            max_bytes: MAX_METADATA_BYTES
        })
    ));
    assert!(
        NewRelation::new(
            NodeId::new(1)?,
            NodeId::new(2)?,
            "r".repeat(MAX_RELATION_NAME_BYTES),
            0.5
        )
        .is_ok()
    );
    assert!(matches!(
        NewRelation::new(
            NodeId::new(1)?,
            NodeId::new(2)?,
            "r".repeat(MAX_RELATION_NAME_BYTES + 1),
            0.5
        ),
        Err(ValidationError::TooLong {
            field: "relation name",
            max_bytes: MAX_RELATION_NAME_BYTES
        })
    ));
    Ok(())
}

#[test]
fn query_hop_seed_and_result_caps_are_enforced() -> TestResult {
    let directory = tempdir()?;
    let repository = Repository::open(directory.path().join("limits.memory3d"))?;
    assert!(
        repository
            .search(&"q".repeat(MAX_QUERY_BYTES), &SearchOptions::default())
            .is_ok()
    );
    let query = "q".repeat(MAX_QUERY_BYTES + 1);
    let error = repository.search(&query, &SearchOptions::default());
    assert!(matches!(
        error,
        Err(memory3d_core::RepositoryError::Validation(
            ValidationError::TooLong {
                field: "query",
                max_bytes: MAX_QUERY_BYTES
            }
        ))
    ));

    for invalid in [
        ActivationOptions {
            hops: MAX_HOPS + 1,
            ..ActivationOptions::default()
        },
        ActivationOptions {
            seed_limit: MAX_SEEDS + 1,
            ..ActivationOptions::default()
        },
        ActivationOptions {
            limit: MAX_RESULTS + 1,
            ..ActivationOptions::default()
        },
    ] {
        assert!(matches!(
            invalid.validate(),
            Err(ValidationError::OutOfRange { .. })
        ));
    }
    assert!(
        ActivationOptions {
            hops: MAX_HOPS,
            seed_limit: MAX_SEEDS,
            limit: MAX_RESULTS,
            ..ActivationOptions::default()
        }
        .validate()
        .is_ok()
    );
    assert!(
        SearchOptions {
            limit: MAX_RESULTS,
            kind: None
        }
        .validate()
        .is_ok()
    );
    assert!(matches!(
        SearchOptions {
            limit: MAX_RESULTS + 1,
            kind: None
        }
        .validate(),
        Err(ValidationError::OutOfRange { field: "limit", .. })
    ));
    Ok(())
}
