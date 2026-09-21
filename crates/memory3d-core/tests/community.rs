//! Task 14 deterministic community organization regression tests.

#[path = "../examples/community_fixture/mod.rs"]
#[allow(dead_code)]
mod community_fixture;

use std::{collections::BTreeMap, error::Error};

use community_fixture::{FixtureKind, InsertionOrder, QUERY};
use memory3d_core::{
    CommunityPolicy, CommunityResolution, CommunityRoute, CommunityStatus, FeedbackKind,
    MemoryKind, NewFeedbackEvent, NewMemory, Repository, RepositoryError,
};
use tempfile::tempdir;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn builder_records_two_resolution_raw_provenance_and_controls() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("controls.memory3d"))?;
    community_fixture::ingest(
        &mut repository,
        FixtureKind::NoisyHub,
        InsertionOrder::Forward,
    )?;
    assert!(matches!(
        repository.community_status()?,
        CommunityStatus::Missing { .. }
    ));
    let report = repository.build_communities(&CommunityPolicy::multi_resolution_v1())?;
    assert_eq!(report.build_node_work, repository.list_nodes()?.len());
    assert_eq!(
        report.build_edge_work,
        repository.list_relations()?.len() * 3
    );
    assert_eq!(report.resolutions.len(), 2);
    assert_eq!(report.memberships.len(), report.build_node_work * 2);
    assert!(
        report
            .resolutions
            .iter()
            .any(|row| row.resolution == CommunityResolution::Fine && row.community_count > 2)
    );
    assert!(
        report
            .resolutions
            .iter()
            .any(|row| row.resolution == CommunityResolution::Coarse && row.community_count > 1)
    );
    assert!(matches!(
        repository.community_status()?,
        CommunityStatus::Current { .. }
    ));

    let activation = repository.activate_with_communities(
        QUERY,
        &community_fixture::options(FixtureKind::NoisyHub),
        &CommunityPolicy::multi_resolution_v1(),
    )?;
    let diagnostics = activation.community.as_ref().ok_or("missing diagnostics")?;
    assert!(diagnostics.fine_routed_edges > 0);
    assert!(diagnostics.coarse_routed_edges > 0 || diagnostics.boundary_routed_edges > 0);
    assert!(
        diagnostics
            .edges
            .iter()
            .any(|edge| edge.route != CommunityRoute::Fine)
    );
    assert_eq!(diagnostics.results.len(), activation.results.len());
    assert!(diagnostics.results.iter().all(|result| {
        result.raw_path_nodes.last() == Some(&result.result)
            && result.memberships.len() == result.raw_path_nodes.len() * 2
    }));
    assert!(community_fixture::paths_valid(
        &repository,
        &activation.results
    )?);
    Ok(())
}

#[test]
fn stale_artifacts_are_rejected_after_node_and_relation_mutations() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("stale.memory3d"))?;
    community_fixture::ingest(
        &mut repository,
        FixtureKind::ParameterSelection,
        InsertionOrder::Forward,
    )?;
    let policy = CommunityPolicy::multi_resolution_v1();
    repository.build_communities(&policy)?;
    repository.add_node(NewMemory::new(
        MemoryKind::new("note")?,
        "Late node invalidates communities",
        0.4,
    )?)?;
    assert!(matches!(
        repository.community_status()?,
        CommunityStatus::Stale { .. }
    ));
    assert!(matches!(
        repository.activate_with_communities(
            QUERY,
            &community_fixture::options(FixtureKind::ParameterSelection),
            &policy
        ),
        Err(RepositoryError::CommunityArtifactsStale { .. })
    ));

    repository.build_communities(&policy)?;
    let relation = repository.list_relations()?.remove(0);
    repository.update_relation_weight(
        relation.source(),
        relation.target(),
        relation.name(),
        (relation.weight() - 0.01).max(0.0),
    )?;
    assert!(matches!(
        repository.activate_with_communities(
            QUERY,
            &community_fixture::options(FixtureKind::ParameterSelection),
            &policy
        ),
        Err(RepositoryError::CommunityArtifactsStale { .. })
    ));
    repository.build_communities(&policy)?;
    assert!(
        repository
            .activate_with_communities(
                QUERY,
                &community_fixture::options(FixtureKind::ParameterSelection),
                &policy
            )?
            .community
            .is_some()
    );
    Ok(())
}

#[test]
fn disable_and_delete_leave_raw_evidence_unchanged() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("rollback.memory3d"))?;
    community_fixture::ingest(
        &mut repository,
        FixtureKind::NoisyHub,
        InsertionOrder::Forward,
    )?;
    let nodes = repository.list_nodes()?;
    let relations = repository.list_relations()?;
    let options = community_fixture::options(FixtureKind::NoisyHub);
    let plain_before = repository.activate(QUERY, &options)?;
    let policy = CommunityPolicy::multi_resolution_v1();
    repository.build_communities(&policy)?;
    repository.activate_with_communities(QUERY, &options, &policy)?;
    let removed = repository.delete_communities()?;
    assert_eq!(removed, nodes.len() * 2);
    assert_eq!(repository.list_nodes()?, nodes);
    assert_eq!(repository.list_relations()?, relations);
    assert_eq!(
        repository.activate(QUERY, &options)?.results,
        plain_before.results
    );
    assert!(matches!(
        repository.activate_with_communities(QUERY, &options, &policy),
        Err(RepositoryError::CommunityArtifactsMissing)
    ));
    Ok(())
}

#[test]
fn insertion_order_preserves_semantic_memberships_and_results() -> TestResult {
    let directory = tempdir()?;
    let mut snapshots = Vec::new();
    for order in [InsertionOrder::Forward, InsertionOrder::Reverse] {
        let mut repository = Repository::open(
            directory
                .path()
                .join(format!("{}.memory3d", order.as_str())),
        )?;
        community_fixture::ingest(&mut repository, FixtureKind::CrossModule, order)?;
        let policy = CommunityPolicy::multi_resolution_v1();
        repository.build_communities(&policy)?;
        let nodes = repository
            .list_nodes()?
            .into_iter()
            .map(|node| (node.id(), node.text().to_owned()))
            .collect::<BTreeMap<_, _>>();
        let mut groups = BTreeMap::<(CommunityResolution, u64), Vec<String>>::new();
        for membership in repository.list_community_memberships()? {
            groups
                .entry((membership.resolution, membership.community_id))
                .or_default()
                .push(nodes[&membership.node].clone());
        }
        let mut semantic_groups = groups.into_values().collect::<Vec<_>>();
        for group in &mut semantic_groups {
            group.sort();
        }
        semantic_groups.sort();
        let activation = repository.activate_with_communities(
            QUERY,
            &community_fixture::options(FixtureKind::CrossModule),
            &policy,
        )?;
        let texts = activation
            .results
            .iter()
            .map(|result| result.node.text().to_owned())
            .collect::<Vec<_>>();
        snapshots.push((semantic_groups, texts));
    }
    assert_eq!(snapshots[0], snapshots[1]);
    Ok(())
}

#[test]
fn frozen_parameters_are_validated() {
    assert!(
        CommunityPolicy {
            fine_weight_threshold: 0.4,
            coarse_weight_threshold: 0.8,
            ..CommunityPolicy::multi_resolution_v1()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn feedback_changes_do_not_stale_query_independent_communities() -> TestResult {
    let directory = tempdir()?;
    let mut repository = Repository::open(directory.path().join("feedback.memory3d"))?;
    community_fixture::ingest(
        &mut repository,
        FixtureKind::NoisyHub,
        InsertionOrder::Forward,
    )?;
    let policy = CommunityPolicy::multi_resolution_v1();
    repository.build_communities(&policy)?;
    let relation = repository
        .list_relations()?
        .into_iter()
        .next()
        .ok_or("missing relation")?;
    repository.record_relation_feedback(NewFeedbackEvent::new(
        relation.source(),
        relation.target(),
        relation.name(),
        FeedbackKind::Positive,
        1.0,
        2_000,
    )?)?;
    assert!(matches!(
        repository.community_status()?,
        CommunityStatus::Current { .. }
    ));
    Ok(())
}

#[test]
fn current_community_artifacts_survive_reopen() -> TestResult {
    let directory = tempdir()?;
    let path = directory.path().join("reopen.memory3d");
    let expected = {
        let mut repository = Repository::open(&path)?;
        community_fixture::ingest(
            &mut repository,
            FixtureKind::NoisyHub,
            InsertionOrder::Forward,
        )?;
        let policy = CommunityPolicy::multi_resolution_v1();
        repository.build_communities(&policy)?;
        repository
            .activate_with_communities(
                QUERY,
                &community_fixture::options(FixtureKind::NoisyHub),
                &policy,
            )?
            .results
    };
    let reopened = Repository::open(path)?;
    assert!(matches!(
        reopened.community_status()?,
        CommunityStatus::Current { .. }
    ));
    let actual = reopened
        .activate_with_communities(
            QUERY,
            &community_fixture::options(FixtureKind::NoisyHub),
            &CommunityPolicy::multi_resolution_v1(),
        )?
        .results;
    assert_eq!(actual, expected);
    Ok(())
}
