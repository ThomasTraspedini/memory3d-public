use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, params};

use crate::{
    MAX_VISITED_EDGES, MemoryNode, NodeId, Repository, RepositoryError, ValidationError,
    repository::{id_from_i64, id_to_i64},
    validate_range,
};

const POLICY_NAME: &str = "multi-resolution-v1";

/// Frozen parameters for deterministic multi-resolution community organization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommunityPolicy {
    /// Minimum raw relation weight used to connect fine communities.
    pub fine_weight_threshold: f32,
    /// Minimum raw relation weight used to connect coarse communities.
    pub coarse_weight_threshold: f32,
    /// Maximum raw outgoing candidates inspected before community routing.
    pub candidate_scan_limit: usize,
}

impl CommunityPolicy {
    /// Task 14's parameter-selection result.
    #[must_use]
    pub const fn multi_resolution_v1() -> Self {
        Self {
            fine_weight_threshold: 0.85,
            coarse_weight_threshold: 0.50,
            candidate_scan_limit: MAX_VISITED_EDGES,
        }
    }

    /// Validates thresholds and the bounded candidate scan.
    ///
    /// # Errors
    ///
    /// Returns a validation error for non-finite/inverted thresholds or an invalid scan limit.
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_range(
            self.candidate_scan_limit,
            "community_candidate_scan_limit",
            1,
            MAX_VISITED_EDGES,
        )?;
        for (field, value) in [
            ("fine_weight_threshold", self.fine_weight_threshold),
            ("coarse_weight_threshold", self.coarse_weight_threshold),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(ValidationError::InvalidNumber {
                    field,
                    min: 0.0,
                    max: 1.0,
                });
            }
        }
        if self.coarse_weight_threshold > self.fine_weight_threshold {
            return Err(ValidationError::InvalidNumber {
                field: "coarse_weight_threshold",
                min: 0.0,
                max: self.fine_weight_threshold,
            });
        }
        Ok(())
    }
}

impl Default for CommunityPolicy {
    fn default() -> Self {
        Self::multi_resolution_v1()
    }
}

/// Resolution of a persisted deterministic community.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommunityResolution {
    /// Strong-edge connected components at the fine threshold.
    Fine,
    /// Broader connected components at the coarse threshold.
    Coarse,
}

impl CommunityResolution {
    pub(crate) const fn storage_value(self) -> i64 {
        match self {
            Self::Fine => 0,
            Self::Coarse => 1,
        }
    }

    fn from_storage(value: i64) -> Result<Self, RepositoryError> {
        match value {
            0 => Ok(Self::Fine),
            1 => Ok(Self::Coarse),
            _ => Err(RepositoryError::InvalidDatabase(format!(
                "invalid community resolution {value}"
            ))),
        }
    }

    /// Stable report label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fine => "fine",
            Self::Coarse => "coarse",
        }
    }
}

/// Raw-node membership in one persisted community resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommunityMembership {
    /// Raw member node.
    pub node: NodeId,
    /// Resolution at which membership was computed.
    pub resolution: CommunityResolution,
    /// Deterministic community number within the build.
    pub community_id: u64,
}

/// Counts for one community resolution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommunityResolutionSummary {
    /// Resolution represented by the row.
    pub resolution: CommunityResolution,
    /// Raw relation-weight threshold.
    pub weight_threshold: f32,
    /// Number of communities, including singleton components.
    pub community_count: usize,
    /// Number of raw memberships.
    pub membership_count: usize,
}

/// Exact deterministic work and provenance emitted by a community build.
#[derive(Debug, Clone, PartialEq)]
pub struct CommunityBuildReport {
    /// Stable policy name.
    pub policy: &'static str,
    /// Raw graph revision captured transactionally.
    pub graph_revision: u64,
    /// Raw nodes inspected.
    pub build_node_work: usize,
    /// Raw relations inspected once per resolution.
    pub build_edge_work: usize,
    /// Per-resolution counts and frozen thresholds.
    pub resolutions: Vec<CommunityResolutionSummary>,
    /// Persisted raw-node memberships.
    pub memberships: Vec<CommunityMembership>,
}

/// Freshness state of the current community overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunityStatus {
    /// No overlay exists.
    Missing {
        /// Current raw graph revision.
        current_revision: u64,
    },
    /// Overlay matches the current raw graph.
    Current {
        /// Shared build/raw revision.
        revision: u64,
    },
    /// Raw evidence changed after the overlay was built.
    Stale {
        /// Overlay build revision.
        built_revision: u64,
        /// Current raw graph revision.
        current_revision: u64,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct CommunityRoutingIndex {
    pub(crate) revision: u64,
    pub(crate) memberships: BTreeMap<NodeId, (u64, u64)>,
    pub(crate) summaries: Vec<CommunityResolutionSummary>,
}

impl CommunityRoutingIndex {
    pub(crate) fn memberships_for(&self, node: NodeId) -> Vec<CommunityMembership> {
        self.memberships
            .get(&node)
            .map_or_else(Vec::new, |(fine, coarse)| {
                vec![
                    CommunityMembership {
                        node,
                        resolution: CommunityResolution::Fine,
                        community_id: *fine,
                    },
                    CommunityMembership {
                        node,
                        resolution: CommunityResolution::Coarse,
                        community_id: *coarse,
                    },
                ]
            })
    }
}

impl Repository {
    /// Builds and atomically replaces the deterministic community overlay.
    ///
    /// The builder reads only raw nodes, raw relation endpoints, and raw relation weights. It has
    /// no query, label, relevance, feedback, or coordinate input.
    ///
    /// # Errors
    ///
    /// Returns a validation, storage, decoding, or clock error.
    pub fn build_communities(
        &mut self,
        policy: &CommunityPolicy,
    ) -> Result<CommunityBuildReport, RepositoryError> {
        policy.validate()?;
        let nodes = self.list_nodes()?;
        let relations = self.list_relations()?;
        let revision = self.graph_revision()?;
        let fine = build_resolution(&nodes, &relations, policy.fine_weight_threshold, true)?;
        let coarse = build_resolution(&nodes, &relations, policy.coarse_weight_threshold, false)?;
        let build_node_work = nodes.len();
        let build_edge_work = relations.len().checked_mul(3).ok_or_else(|| {
            RepositoryError::InvalidDatabase("community build work overflow".to_owned())
        })?;
        let now = super::repository::unix_time_ms()?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "DELETE FROM community_builds WHERE policy = ?1",
            [POLICY_NAME],
        )?;
        transaction.execute(
            "INSERT INTO community_builds (policy, graph_revision, built_at_ms, fine_threshold, coarse_threshold, build_node_work, build_edge_work) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                POLICY_NAME,
                u64_to_i64(revision)?,
                now,
                policy.fine_weight_threshold,
                policy.coarse_weight_threshold,
                usize_to_i64(build_node_work)?,
                usize_to_i64(build_edge_work)?
            ],
        )?;
        let mut memberships = Vec::with_capacity(nodes.len().saturating_mul(2));
        for (resolution, rows) in [
            (CommunityResolution::Fine, &fine),
            (CommunityResolution::Coarse, &coarse),
        ] {
            for (&node, &community_id) in rows {
                transaction.execute(
                    "INSERT INTO community_memberships (policy, resolution, community_id, node_id) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        POLICY_NAME,
                        resolution.storage_value(),
                        u64_to_i64(community_id)?,
                        id_to_i64(node)?
                    ],
                )?;
                memberships.push(CommunityMembership {
                    node,
                    resolution,
                    community_id,
                });
            }
        }
        transaction.commit()?;
        memberships.sort_by_key(|row| (row.resolution, row.community_id, row.node));
        let resolutions = vec![
            resolution_summary(
                CommunityResolution::Fine,
                policy.fine_weight_threshold,
                &fine,
            ),
            resolution_summary(
                CommunityResolution::Coarse,
                policy.coarse_weight_threshold,
                &coarse,
            ),
        ];
        Ok(CommunityBuildReport {
            policy: POLICY_NAME,
            graph_revision: revision,
            build_node_work,
            build_edge_work,
            resolutions,
            memberships,
        })
    }

    /// Reports whether persisted communities are absent, current, or stale.
    ///
    /// # Errors
    ///
    /// Returns a database error or rejects malformed stored counters.
    pub fn community_status(&self) -> Result<CommunityStatus, RepositoryError> {
        let current_revision = self.graph_revision()?;
        let built = self
            .connection
            .query_row(
                "SELECT graph_revision FROM community_builds WHERE policy = ?1",
                [POLICY_NAME],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(i64_to_u64)
            .transpose()?;
        Ok(match built {
            None => CommunityStatus::Missing { current_revision },
            Some(built_revision) if built_revision == current_revision => {
                CommunityStatus::Current {
                    revision: current_revision,
                }
            }
            Some(built_revision) => CommunityStatus::Stale {
                built_revision,
                current_revision,
            },
        })
    }

    /// Lists persisted memberships in deterministic resolution/community/member order.
    ///
    /// # Errors
    ///
    /// Returns a database or malformed-data error.
    pub fn list_community_memberships(&self) -> Result<Vec<CommunityMembership>, RepositoryError> {
        let mut statement = self.connection.prepare(
            "SELECT resolution, community_id, node_id FROM community_memberships WHERE policy = ?1 ORDER BY resolution, community_id, node_id",
        )?;
        let rows = statement.query_map([POLICY_NAME], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (resolution, community_id, node_id) = row?;
            Ok(CommunityMembership {
                node: id_from_i64(node_id)?,
                resolution: CommunityResolution::from_storage(resolution)?,
                community_id: i64_to_u64(community_id)?,
            })
        })
        .collect()
    }

    /// Deletes only derived community artifacts and returns the number of removed memberships.
    ///
    /// Raw nodes and relations are never changed.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub fn delete_communities(&mut self) -> Result<usize, RepositoryError> {
        let transaction = self.connection.transaction()?;
        let removed = transaction.query_row(
            "SELECT count(*) FROM community_memberships WHERE policy = ?1",
            [POLICY_NAME],
            |row| row.get::<_, i64>(0),
        )?;
        transaction.execute(
            "DELETE FROM community_builds WHERE policy = ?1",
            [POLICY_NAME],
        )?;
        transaction.commit()?;
        usize::try_from(removed).map_err(|_| {
            RepositoryError::InvalidDatabase("community membership count is invalid".to_owned())
        })
    }

    pub(crate) fn community_routing_index(
        &self,
        policy: CommunityPolicy,
    ) -> Result<CommunityRoutingIndex, RepositoryError> {
        let current_revision = self.graph_revision()?;
        let build = self
            .connection
            .query_row(
                "SELECT graph_revision, fine_threshold, coarse_threshold FROM community_builds WHERE policy = ?1",
                [POLICY_NAME],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, f32>(1)?, row.get::<_, f32>(2)?)),
            )
            .optional()?
            .ok_or(RepositoryError::CommunityArtifactsMissing)?;
        let built_revision = i64_to_u64(build.0)?;
        if built_revision != current_revision {
            return Err(RepositoryError::CommunityArtifactsStale {
                built_revision,
                current_revision,
            });
        }
        if build.1.to_bits() != policy.fine_weight_threshold.to_bits()
            || build.2.to_bits() != policy.coarse_weight_threshold.to_bits()
        {
            return Err(RepositoryError::InvalidDatabase(
                "community build parameters do not match activation policy".to_owned(),
            ));
        }
        let rows = self.list_community_memberships()?;
        let mut memberships = BTreeMap::<NodeId, (Option<u64>, Option<u64>)>::new();
        for row in rows {
            let entry = memberships.entry(row.node).or_default();
            match row.resolution {
                CommunityResolution::Fine => entry.0 = Some(row.community_id),
                CommunityResolution::Coarse => entry.1 = Some(row.community_id),
            }
        }
        let memberships = memberships
            .into_iter()
            .map(|(node, (fine, coarse))| {
                fine.zip(coarse).map(|pair| (node, pair)).ok_or_else(|| {
                    RepositoryError::InvalidDatabase(format!(
                        "node {node} lacks one community resolution"
                    ))
                })
            })
            .collect::<Result<_, _>>()?;
        let summaries = vec![
            resolution_summary_from_memberships(
                CommunityResolution::Fine,
                policy.fine_weight_threshold,
                &memberships,
            ),
            resolution_summary_from_memberships(
                CommunityResolution::Coarse,
                policy.coarse_weight_threshold,
                &memberships,
            ),
        ];
        Ok(CommunityRoutingIndex {
            revision: current_revision,
            memberships,
            summaries,
        })
    }

    fn graph_revision(&self) -> Result<u64, RepositoryError> {
        let value = self.connection.query_row(
            "SELECT graph_revision FROM community_graph_state WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        i64_to_u64(value)
    }
}

fn build_resolution(
    nodes: &[MemoryNode],
    relations: &[crate::Relation],
    threshold: f32,
    reciprocal_only: bool,
) -> Result<BTreeMap<NodeId, u64>, RepositoryError> {
    let mut ordered = nodes.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        left.kind()
            .as_str()
            .cmp(right.kind().as_str())
            .then_with(|| left.text().cmp(right.text()))
            .then_with(|| left.id().cmp(&right.id()))
    });
    let indexes = ordered
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id(), index))
        .collect::<BTreeMap<_, _>>();
    let reciprocal_edges = if reciprocal_only {
        relations
            .iter()
            .filter(|relation| relation.weight() >= threshold)
            .map(|relation| (relation.source(), relation.target()))
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    let mut parents = (0..ordered.len()).collect::<Vec<_>>();
    for relation in relations {
        if relation.weight() >= threshold
            && (!reciprocal_only
                || reciprocal_edges.contains(&(relation.target(), relation.source())))
        {
            let left = *indexes.get(&relation.source()).ok_or_else(|| {
                RepositoryError::InvalidDatabase("community source node is missing".to_owned())
            })?;
            let right = *indexes.get(&relation.target()).ok_or_else(|| {
                RepositoryError::InvalidDatabase("community target node is missing".to_owned())
            })?;
            union(&mut parents, left, right);
        }
    }
    let mut components = BTreeMap::<usize, Vec<NodeId>>::new();
    for (index, node) in ordered.iter().enumerate() {
        let root = find(&mut parents, index);
        components.entry(root).or_default().push(node.id());
    }
    let mut groups = components.into_values().collect::<Vec<_>>();
    groups.sort_by_key(|members| {
        members
            .iter()
            .filter_map(|id| indexes.get(id).copied())
            .min()
            .unwrap_or(usize::MAX)
    });
    let mut result = BTreeMap::new();
    for (index, members) in groups.into_iter().enumerate() {
        let community_id = u64::try_from(index + 1).map_err(|_| {
            RepositoryError::InvalidDatabase("community count exceeds storage range".to_owned())
        })?;
        for node in members {
            result.insert(node, community_id);
        }
    }
    Ok(result)
}

fn find(parents: &mut [usize], node: usize) -> usize {
    let mut root = node;
    while parents[root] != root {
        root = parents[root];
    }
    let mut current = node;
    while parents[current] != current {
        let next = parents[current];
        parents[current] = root;
        current = next;
    }
    root
}

fn union(parents: &mut [usize], left: usize, right: usize) {
    let left_root = find(parents, left);
    let right_root = find(parents, right);
    if left_root != right_root {
        let (keep, merge) = if left_root < right_root {
            (left_root, right_root)
        } else {
            (right_root, left_root)
        };
        parents[merge] = keep;
    }
}

fn resolution_summary(
    resolution: CommunityResolution,
    threshold: f32,
    memberships: &BTreeMap<NodeId, u64>,
) -> CommunityResolutionSummary {
    CommunityResolutionSummary {
        resolution,
        weight_threshold: threshold,
        community_count: memberships.values().copied().collect::<BTreeSet<_>>().len(),
        membership_count: memberships.len(),
    }
}

fn resolution_summary_from_memberships(
    resolution: CommunityResolution,
    threshold: f32,
    memberships: &BTreeMap<NodeId, (u64, u64)>,
) -> CommunityResolutionSummary {
    let ids = memberships
        .values()
        .map(|pair| match resolution {
            CommunityResolution::Fine => pair.0,
            CommunityResolution::Coarse => pair.1,
        })
        .collect::<BTreeSet<_>>();
    CommunityResolutionSummary {
        resolution,
        weight_threshold: threshold,
        community_count: ids.len(),
        membership_count: memberships.len(),
    }
}

fn usize_to_i64(value: usize) -> Result<i64, RepositoryError> {
    i64::try_from(value)
        .map_err(|_| RepositoryError::InvalidDatabase("counter exceeds SQLite range".to_owned()))
}

fn u64_to_i64(value: u64) -> Result<i64, RepositoryError> {
    i64::try_from(value)
        .map_err(|_| RepositoryError::InvalidDatabase("revision exceeds SQLite range".to_owned()))
}

fn i64_to_u64(value: i64) -> Result<u64, RepositoryError> {
    u64::try_from(value)
        .map_err(|_| RepositoryError::InvalidDatabase("negative stored counter".to_owned()))
}
