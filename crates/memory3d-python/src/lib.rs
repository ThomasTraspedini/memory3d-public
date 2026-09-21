//! Thin `PyO3` adapter around the deterministic `memory3d-core` API.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use memory3d_core::{
    ActivationOptions, ActivationReport, DatabaseStats, MemoryKind, MemoryNode as CoreMemoryNode,
    Metadata, NewMemory, NewRelation, NodeId, PathStep as CorePathStep, Relation as CoreRelation,
    Repository, RepositoryError, SearchOptions, SearchResult as CoreSearchResult, TraversalStats,
};
use pyo3::{
    exceptions::PyRuntimeError,
    prelude::*,
    types::{PyAny, PyModule, PyType},
};
use serde_json::Value;

#[allow(missing_docs)]
mod exceptions {
    use pyo3::{create_exception, exceptions::PyException};

    create_exception!(memory3d, Memory3DError, PyException);
    create_exception!(memory3d, ValidationError, Memory3DError);
    create_exception!(memory3d, DatabaseError, Memory3DError);
    create_exception!(memory3d, BusyError, Memory3DError);
    create_exception!(memory3d, NotFoundError, Memory3DError);
    create_exception!(memory3d, ClosedError, Memory3DError);
}

use exceptions::{
    BusyError, ClosedError, DatabaseError, Memory3DError, NotFoundError, ValidationError,
};

/// Optional coordinates stored with a memory node.
#[pyclass(frozen, get_all, module = "memory3d")]
#[derive(Clone)]
struct Coordinates {
    x: f64,
    y: f64,
    z: f64,
}

/// Stored text memory returned by the core repository.
#[pyclass(frozen, module = "memory3d")]
#[derive(Clone)]
struct MemoryNode {
    #[pyo3(get)]
    id: u128,
    #[pyo3(get)]
    kind: String,
    #[pyo3(get)]
    text: String,
    #[pyo3(get)]
    importance: f32,
    metadata: Value,
    #[pyo3(get)]
    coordinates: Option<Coordinates>,
    #[pyo3(get)]
    created_at_ms: i64,
    #[pyo3(get)]
    updated_at_ms: i64,
}

#[pymethods]
impl MemoryNode {
    #[getter]
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_python(py, &self.metadata)
    }

    fn as_dict(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_python(
            py,
            &serde_json::json!({
                "id": self.id,
                "kind": self.kind,
                "text": self.text,
                "importance": self.importance,
                "metadata": self.metadata,
                "coordinates": self.coordinates.as_ref().map(|coordinates| serde_json::json!({
                    "x": coordinates.x,
                    "y": coordinates.y,
                    "z": coordinates.z,
                })),
                "created_at_ms": self.created_at_ms,
                "updated_at_ms": self.updated_at_ms,
            }),
        )
    }

    fn __repr__(&self) -> String {
        format!(
            "MemoryNode(id={}, kind={:?}, importance={})",
            self.id, self.kind, self.importance
        )
    }
}

/// Directed relation between two memory nodes.
#[pyclass(frozen, get_all, name = "Relation", module = "memory3d")]
#[derive(Clone)]
struct PyRelation {
    source: u128,
    target: u128,
    relation: String,
    weight: f32,
    reinforcement_count: u64,
    created_at_ms: i64,
    updated_at_ms: i64,
}

#[pymethods]
impl PyRelation {
    fn as_dict(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_python(
            py,
            &serde_json::json!({
                "source": self.source,
                "target": self.target,
                "relation": self.relation,
                "weight": self.weight,
                "reinforcement_count": self.reinforcement_count,
                "created_at_ms": self.created_at_ms,
                "updated_at_ms": self.updated_at_ms,
            }),
        )
    }

    fn __repr__(&self) -> String {
        format!(
            "Relation(source={}, relation={:?}, target={}, weight={})",
            self.source, self.relation, self.target, self.weight
        )
    }
}

/// One lexical search match.
#[pyclass(frozen, get_all, name = "ActivationReport", module = "memory3d")]
#[derive(Clone)]
struct SearchResult {
    node: MemoryNode,
    matched_terms: usize,
    score: f32,
}

/// One relation step in an activation path.
#[pyclass(frozen, get_all, module = "memory3d")]
#[derive(Clone)]
struct PathStep {
    source: u128,
    relation: String,
    weight: f32,
    target: u128,
}

/// Ordered path from lexical seed to activated node.
#[pyclass(frozen, get_all, module = "memory3d")]
#[derive(Clone)]
struct ActivationPath {
    seed: u128,
    steps: Vec<PathStep>,
}

/// One ranked activation result.
#[pyclass(frozen, get_all, module = "memory3d")]
#[derive(Clone)]
struct ActivationResult {
    node: MemoryNode,
    score: f32,
    hops: usize,
    path: ActivationPath,
}

/// Work consumed by one activation.
#[pyclass(frozen, get_all, module = "memory3d")]
#[derive(Clone)]
struct ActivationStats {
    seeds: usize,
    visited_nodes: usize,
    visited_edges: usize,
}

/// Database reads issued by one activation.
#[pyclass(frozen, get_all, module = "memory3d")]
#[derive(Clone)]
struct ActivationDatabaseStats {
    seed_queries: usize,
    seed_node_reads: usize,
    adjacency_queries: usize,
    traversal_node_reads: usize,
}

/// Diagnostic wall-clock timings for one activation, in microseconds.
#[pyclass(frozen, get_all, module = "memory3d")]
#[derive(Clone)]
struct ActivationTimings {
    seed_lookup_us: u64,
    graph_us: u64,
}

/// Ranked activation results plus traversal counters.
#[pyclass(frozen, get_all, module = "memory3d")]
#[derive(Clone)]
struct ActivationReportPy {
    results: Vec<ActivationResult>,
    stats: ActivationStats,
    database: ActivationDatabaseStats,
    adjacency_sources: Vec<u128>,
    timings: ActivationTimings,
}

/// Open local `Memory3D` database.
#[pyclass(unsendable, module = "memory3d")]
struct Memory {
    path: PathBuf,
    repository: Option<Repository>,
}

#[pymethods]
impl Memory {
    #[new]
    fn new(path: PathBuf) -> PyResult<Self> {
        Self::open_path(path)
    }

    #[classmethod]
    fn open(_cls: &Bound<'_, PyType>, path: PathBuf) -> PyResult<Self> {
        Self::open_path(path)
    }

    fn close(&mut self) {
        self.repository = None;
    }

    fn __enter__(slf: PyRefMut<'_, Self>) -> PyRefMut<'_, Self> {
        slf
    }

    fn __exit__(
        &mut self,
        _exc_type: &Bound<'_, PyAny>,
        _exc_value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> bool {
        self.close();
        false
    }

    #[pyo3(signature = (kind, text, importance=0.5, metadata=None, coordinates=None))]
    fn add_text(
        &mut self,
        kind: String,
        text: String,
        importance: f32,
        metadata: Option<&Bound<'_, PyAny>>,
        coordinates: Option<(f64, f64, f64)>,
    ) -> PyResult<MemoryNode> {
        let kind = MemoryKind::new(kind).map_err(py_validation_error)?;
        let mut memory = NewMemory::new(kind, text, importance).map_err(py_validation_error)?;
        let metadata = metadata_from_python(metadata)?;
        if metadata != Metadata::default() {
            memory = memory.with_metadata(metadata);
        }
        if let Some((x, y, z)) = coordinates {
            let coordinates =
                memory3d_core::Coordinates::new(x, y, z).map_err(py_validation_error)?;
            memory = memory.with_coordinates(coordinates);
        }
        let node = self
            .repository_mut()?
            .add_node(memory)
            .map_err(py_repository_error)?;
        Ok(node.into())
    }

    #[pyo3(signature = (source, target, relation, weight=1.0))]
    fn link(
        &mut self,
        source: u128,
        target: u128,
        relation: String,
        weight: f32,
    ) -> PyResult<PyRelation> {
        let relation = NewRelation::new(
            NodeId::new(source).map_err(py_validation_error)?,
            NodeId::new(target).map_err(py_validation_error)?,
            relation,
            weight,
        )
        .map_err(py_validation_error)?;
        Ok(self
            .repository_mut()?
            .add_relation(relation)
            .map_err(py_repository_error)?
            .into())
    }

    fn get(&self, id: u128) -> PyResult<Option<MemoryNode>> {
        Ok(self
            .repository()?
            .get_node(NodeId::new(id).map_err(py_validation_error)?)
            .map_err(py_repository_error)?
            .map(MemoryNode::from))
    }

    #[pyo3(signature = (query, limit=10, kind=None))]
    fn search(
        &self,
        query: &str,
        limit: usize,
        kind: Option<String>,
    ) -> PyResult<Vec<SearchResult>> {
        let kind = kind
            .map(MemoryKind::new)
            .transpose()
            .map_err(py_validation_error)?;
        Ok(self
            .repository()?
            .search(query, &SearchOptions { limit, kind })
            .map_err(py_repository_error)?
            .into_iter()
            .map(SearchResult::from)
            .collect())
    }

    #[pyo3(signature = (
        query,
        hops=None,
        seed_limit=None,
        limit=None,
        max_visited_nodes=None,
        max_visited_edges=None,
        include_seeds=false
    ))]
    #[allow(clippy::too_many_arguments)]
    fn activate(
        &self,
        query: &str,
        hops: Option<u8>,
        seed_limit: Option<usize>,
        limit: Option<usize>,
        max_visited_nodes: Option<usize>,
        max_visited_edges: Option<usize>,
        include_seeds: bool,
    ) -> PyResult<ActivationReportPy> {
        let defaults = ActivationOptions::default();
        let options = ActivationOptions {
            hops: hops.unwrap_or(defaults.hops),
            seed_limit: seed_limit.unwrap_or(defaults.seed_limit),
            limit: limit.unwrap_or(defaults.limit),
            max_visited_nodes: max_visited_nodes.unwrap_or(defaults.max_visited_nodes),
            max_visited_edges: max_visited_edges.unwrap_or(defaults.max_visited_edges),
            include_seeds,
        };
        Ok(self
            .repository()?
            .activate(query, &options)
            .map_err(py_repository_error)?
            .into())
    }

    #[getter]
    fn path(&self) -> String {
        self.path.display().to_string()
    }

    #[getter]
    fn closed(&self) -> bool {
        self.repository.is_none()
    }

    fn __repr__(&self) -> String {
        format!("Memory(path={:?}, closed={})", self.path(), self.closed())
    }
}

impl Memory {
    fn open_path(path: PathBuf) -> PyResult<Self> {
        let repository = Repository::open(&path).map_err(py_repository_error)?;
        Ok(Self {
            path,
            repository: Some(repository),
        })
    }

    fn repository(&self) -> PyResult<&Repository> {
        self.repository
            .as_ref()
            .ok_or_else(|| ClosedError::new_err("Memory database is closed"))
    }

    fn repository_mut(&mut self) -> PyResult<&mut Repository> {
        self.repository
            .as_mut()
            .ok_or_else(|| ClosedError::new_err("Memory database is closed"))
    }
}

impl From<CoreMemoryNode> for MemoryNode {
    fn from(value: CoreMemoryNode) -> Self {
        Self {
            id: value.id().get(),
            kind: value.kind().as_str().to_owned(),
            text: value.text().to_owned(),
            importance: value.importance(),
            metadata: Value::Object(value.metadata().as_object().clone()),
            coordinates: value.coordinates().map(|coordinates| Coordinates {
                x: coordinates.x(),
                y: coordinates.y(),
                z: coordinates.z(),
            }),
            created_at_ms: value.created_at_ms(),
            updated_at_ms: value.updated_at_ms(),
        }
    }
}

impl From<CoreRelation> for PyRelation {
    fn from(value: CoreRelation) -> Self {
        Self {
            source: value.source().get(),
            target: value.target().get(),
            relation: value.name().to_owned(),
            weight: value.weight(),
            reinforcement_count: value.reinforcement_count(),
            created_at_ms: value.created_at_ms(),
            updated_at_ms: value.updated_at_ms(),
        }
    }
}

impl From<CorePathStep> for PathStep {
    fn from(value: CorePathStep) -> Self {
        Self {
            source: value.source.get(),
            relation: value.relation,
            weight: value.weight,
            target: value.target.get(),
        }
    }
}

impl From<CoreSearchResult> for SearchResult {
    fn from(value: CoreSearchResult) -> Self {
        Self {
            node: value.node.into(),
            matched_terms: value.matched_terms,
            score: value.score,
        }
    }
}

impl From<TraversalStats> for ActivationStats {
    fn from(value: TraversalStats) -> Self {
        Self {
            seeds: value.seeds,
            visited_nodes: value.visited_nodes,
            visited_edges: value.visited_edges,
        }
    }
}

impl From<DatabaseStats> for ActivationDatabaseStats {
    fn from(value: DatabaseStats) -> Self {
        Self {
            seed_queries: value.seed_queries,
            seed_node_reads: value.seed_node_reads,
            adjacency_queries: value.adjacency_queries,
            traversal_node_reads: value.traversal_node_reads,
        }
    }
}

impl From<ActivationReport> for ActivationReportPy {
    fn from(value: ActivationReport) -> Self {
        let results = value
            .results
            .into_iter()
            .map(|result| {
                let hops = result.hops();
                ActivationResult {
                    node: result.node.into(),
                    score: result.score,
                    hops,
                    path: ActivationPath {
                        seed: result.path.seed.get(),
                        steps: result.path.steps.into_iter().map(PathStep::from).collect(),
                    },
                }
            })
            .collect();
        Self {
            results,
            stats: value.stats.into(),
            database: value.database.into(),
            adjacency_sources: value
                .adjacency_sources
                .into_iter()
                .map(memory3d_core::NodeId::get)
                .collect(),
            timings: ActivationTimings {
                seed_lookup_us: duration_micros(value.timings.seed_lookup),
                graph_us: duration_micros(value.timings.graph),
            },
        }
    }
}

#[allow(clippy::needless_pass_by_value)]
fn py_validation_error(value: memory3d_core::ValidationError) -> PyErr {
    ValidationError::new_err(value.to_string())
}

fn py_repository_error(value: RepositoryError) -> PyErr {
    match value {
        RepositoryError::Validation(error) => ValidationError::new_err(error.to_string()),
        RepositoryError::Evidence(_)
        | RepositoryError::EvidenceIdExists(_)
        | RepositoryError::EvidenceReferenceNotFound { .. }
        | RepositoryError::EvidenceConflictGroupMismatch { .. }
        | RepositoryError::IdempotencyConflict(_)
        | RepositoryError::EvidenceBundleRolledBack(_)
        | RepositoryError::EvidenceRollbackBlocked { .. } => {
            ValidationError::new_err(value.to_string())
        }
        RepositoryError::Busy => BusyError::new_err(value.to_string()),
        RepositoryError::NodeNotFound(_)
        | RepositoryError::RelationNotFound { .. }
        | RepositoryError::FeedbackEventNotFound(_)
        | RepositoryError::EvidenceBundleNotFound(_) => NotFoundError::new_err(value.to_string()),
        RepositoryError::Database(_)
        | RepositoryError::InvalidDatabase(_)
        | RepositoryError::UnsupportedSchema { .. }
        | RepositoryError::CommunityArtifactsMissing
        | RepositoryError::CommunityArtifactsStale { .. }
        | RepositoryError::Clock(_) => DatabaseError::new_err(value.to_string()),
    }
}

fn metadata_from_python(value: Option<&Bound<'_, PyAny>>) -> PyResult<Metadata> {
    let Some(value) = value else {
        return Ok(Metadata::default());
    };
    if value.is_none() {
        return Ok(Metadata::default());
    }
    let json = value.py().import("json")?;
    let encoded: String = json.call_method1("dumps", (value,))?.extract()?;
    let parsed = serde_json::from_str(&encoded).map_err(|error| {
        ValidationError::new_err(format!("metadata must be a JSON object: {error}"))
    })?;
    Metadata::new(parsed).map_err(py_validation_error)
}

fn json_to_python(py: Python<'_>, value: &Value) -> PyResult<Py<PyAny>> {
    let encoded = serde_json::to_string(value)
        .map_err(|error| PyRuntimeError::new_err(format!("failed to encode JSON: {error}")))?;
    let json = py.import("json")?;
    Ok(json.call_method1("loads", (encoded,))?.unbind())
}

fn duration_micros(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).map_or(u64::MAX, |value| value)
}

/// Python bindings for `Memory3D`.
#[pymodule(gil_used = false)]
fn memory3d(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<Memory>()?;
    m.add_class::<Coordinates>()?;
    m.add_class::<MemoryNode>()?;
    m.add_class::<PyRelation>()?;
    m.add_class::<SearchResult>()?;
    m.add_class::<PathStep>()?;
    m.add_class::<ActivationPath>()?;
    m.add_class::<ActivationResult>()?;
    m.add_class::<ActivationStats>()?;
    m.add_class::<ActivationDatabaseStats>()?;
    m.add_class::<ActivationTimings>()?;
    m.add_class::<ActivationReportPy>()?;
    m.add("Memory3DError", m.py().get_type::<Memory3DError>())?;
    m.add("ValidationError", m.py().get_type::<ValidationError>())?;
    m.add("DatabaseError", m.py().get_type::<DatabaseError>())?;
    m.add("BusyError", m.py().get_type::<BusyError>())?;
    m.add("NotFoundError", m.py().get_type::<NotFoundError>())?;
    m.add("ClosedError", m.py().get_type::<ClosedError>())?;
    m.add("SearchResult", m.py().get_type::<SearchResult>())?;
    m.add(
        "__all__",
        vec![
            "__version__",
            "Memory",
            "Coordinates",
            "MemoryNode",
            "Relation",
            "SearchResult",
            "PathStep",
            "ActivationPath",
            "ActivationResult",
            "ActivationStats",
            "ActivationDatabaseStats",
            "ActivationTimings",
            "ActivationReport",
            "Memory3DError",
            "ValidationError",
            "DatabaseError",
            "BusyError",
            "NotFoundError",
            "ClosedError",
        ],
    )?;
    Ok(())
}
