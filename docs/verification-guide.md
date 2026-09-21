# Public verification guide

This guide maps major public claims to implementation, tests, decisions, observable evidence, and
known limits. This document is an index to evidence, not evidence itself. Each claim below is linked
to its implementation, tests, decisions, observable evidence, and known limitations.

## How to verify

Run the repository gates from the root:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Then run the two-process [lifecycle demo](memory-lifecycle.md). Use `--json` before `demo` when
comparing structured output. These checks require no network service or model.

## Claim map

| Claim | Implementation | Deterministic tests | Decision | Demo or research evidence | Known limitation |
| --- | --- | --- | --- | --- | --- |
| **Persistence across reopen.** Nodes, relations, and evidence survive closing and reopening the SQLite file with stable IDs. | [`repository.rs`](../crates/memory3d-core/src/repository.rs), [`evidence.rs`](../crates/memory3d-core/src/evidence.rs) | [`repository.rs` tests](../crates/memory3d-core/tests/repository.rs), [`evidence.rs` tests](../crates/memory3d-core/tests/evidence.rs) | [ADR 0015](decisions/0015-freeze-storage-decision-gate.md) | [Lifecycle demo](memory-lifecycle.md) uses separate ingest and recall processes. | Local SQLite and one-writer assumptions; not distributed durability. |
| **Bounded explainable retrieval.** Activation validates seed, hop, result, visited-node, and visited-edge limits and reports consumed work. | [`retrieval.rs`](../crates/memory3d-core/src/retrieval.rs) | [`activation.rs`](../crates/memory3d-core/tests/activation.rs), especially hard-budget and adversarial-candidate cases | [ADR 0002](decisions/0002-test-scale-aware-associative-growth.md), [ADR 0005](decisions/0005-order-bounded-candidates-by-evidence.md) | [Retrieval record](research/retrieval.md); lifecycle output prints traversal work. | Bounds are not exhaustive retrieval or universal latency guarantees. |
| **Deterministic activation ordering.** Results order by score descending, importance descending, kind ascending, text ascending, then node ID ascending. | Result comparator in [`retrieval.rs`](../crates/memory3d-core/src/retrieval.rs) | Equal-score and insertion-order cases in [`activation.rs`](../crates/memory3d-core/tests/activation.rs) | [ADR 0005](decisions/0005-order-bounded-candidates-by-evidence.md) | [Architecture](architecture.md) records the exact order. | Exact ties can reflect stable ID assignment across separately built databases. |
| **Reconstructable paths.** Every activation result carries its lexical seed and ordered relation steps to the result node. | Path construction and validation in [`retrieval.rs`](../crates/memory3d-core/src/retrieval.rs) and [`lib.rs`](../crates/memory3d-core/src/lib.rs) | Direct, indirect, best-path, and adapter parity cases in [`activation.rs`](../crates/memory3d-core/tests/activation.rs), [`demo_e2e.rs`](../crates/memory3d-cli/tests/demo_e2e.rs), and [`protocol.rs`](../crates/memory3d-mcp/tests/protocol.rs) | [ADR 0002](decisions/0002-test-scale-aware-associative-growth.md) | [Lifecycle demo](memory-lifecycle.md) prints every selected path. | A path explains this algorithm's selection; it does not prove truth or causality. |
| **Evidence lifecycle.** Versioned bundles preserve immutable IDs, lifecycle, source, producer, scope, derivation, and decision records through preview, transactional apply, lookup, replay, and guarded rollback. | [`evidence.rs`](../crates/memory3d-core/src/evidence.rs), schema in [`repository.rs`](../crates/memory3d-core/src/repository.rs) | Preview/apply/reopen/idempotency/atomicity/rollback cases in [`evidence.rs` tests](../crates/memory3d-core/tests/evidence.rs) | [ADR 0021](decisions/0021-adopt-bounded-evidence-envelope.md) | [Evidence envelope](evidence-envelope.md), [lifecycle demo](memory-lifecycle.md) | Records are caller-authored; the core does not verify their factual correctness. |
| **Exact scope filtering.** Missing or mismatched selectors exclude an evidence candidate from context under the `settled` policy. | `settled` policy and context assembly in [`evidence.rs`](../crates/memory3d-core/src/evidence.rs) | Exact-scope Kafka/filesystem regression in [`evidence.rs` tests](../crates/memory3d-core/tests/evidence.rs); MCP mismatch case in [`protocol.rs`](../crates/memory3d-mcp/tests/protocol.rs) | [ADR 0021](decisions/0021-adopt-bounded-evidence-envelope.md) | [Epistemic-safety failure](research/epistemic-safety.md); nursery exclusion in the [lifecycle demo](memory-lifecycle.md) | Scope is exact and caller-declared; it is not inferred, widened, or validated against the world. |
| **Supersession.** Later evidence can explicitly supersede earlier evidence; the earlier item remains stored and visible but is not admitted under the `settled` policy. | Reference storage and `superseded_by` lookup in [`evidence.rs`](../crates/memory3d-core/src/evidence.rs) | Conflict/supersession and lifecycle-demo cases in [`evidence.rs` tests](../crates/memory3d-core/tests/evidence.rs) and [`demo_e2e.rs`](../crates/memory3d-cli/tests/demo_e2e.rs) | [ADR 0021](decisions/0021-adopt-bounded-evidence-envelope.md) | Bedroom readings and recommendations in the [lifecycle demo](memory-lifecycle.md) | The caller declares supersession; the core does not infer recency or correctness. |
| **Conflict handling.** All visible sides of an unresolved declared conflict are excluded; no automatic winner is selected. | Conflict references and policy evaluation in [`evidence.rs`](../crates/memory3d-core/src/evidence.rs) | Conflict visibility in [`evidence.rs` tests](../crates/memory3d-core/tests/evidence.rs) and ventilation case in [`demo_e2e.rs`](../crates/memory3d-cli/tests/demo_e2e.rs) | [ADR 0021](decisions/0021-adopt-bounded-evidence-envelope.md) | Unresolved ventilation query in the [lifecycle demo](memory-lifecycle.md) | Conflicts are explicit records; the core neither discovers nor resolves them automatically. |
| **Bounded candidate scanning.** Evidence context scans ranked activation candidates under a separate limit and reports candidate work, evidence evaluations, ordinary skips, and stop reason. | Context options and assembly loop in [`evidence.rs`](../crates/memory3d-core/src/evidence.rs) | Scan-limit, starvation, exhaustion, result-limit, byte-limit, and traversal-exhaustion cases in [`evidence.rs` tests](../crates/memory3d-core/tests/evidence.rs) | [ADR 0021](decisions/0021-adopt-bounded-evidence-envelope.md) | Candidate counters in the [lifecycle demo](memory-lifecycle.md) | Eligible evidence can remain beyond either retrieval or scan bounds. |
| **Explicit abstention.** `abstained=true` exactly when the bounded scan admits no evidence. | `ContextPackage` construction in [`evidence.rs`](../crates/memory3d-core/src/evidence.rs) | Scope and scan-bound abstention cases in [`evidence.rs` tests](../crates/memory3d-core/tests/evidence.rs) and [`protocol.rs`](../crates/memory3d-mcp/tests/protocol.rs) | [ADR 0021](decisions/0021-adopt-bounded-evidence-envelope.md) | Ventilation conflict in the [lifecycle demo](memory-lifecycle.md) | It is not proof of global absence outside the bounds. |
| **Adapters reuse the same core.** CLI, Python, and MCP translate public inputs and outputs around `memory3d-core`; plain memory is on all four surfaces, evidence on core/CLI/MCP, and feedback on core/CLI. | Adapter crates under [`crates/`](../crates) depend on `memory3d-core`; workspace manifests define the dependency direction. | CLI parity in [`demo_e2e.rs`](../crates/memory3d-cli/tests/demo_e2e.rs), Python/core/CLI parity in [`test_memory3d.py`](../crates/memory3d-python/tests/test_memory3d.py), MCP/core parity in [`protocol.rs`](../crates/memory3d-mcp/tests/protocol.rs) | Architecture boundary; no separate adapter ADR. | [Lifecycle demo](memory-lifecycle.md) exercises CLI over the core; adapter tests inspect parity. | Adapter surfaces are intentionally asymmetric; Python has no evidence-envelope API and MCP has no feedback API. |

## Reading status correctly

- **Implementation fact** means the referenced public code has the described shape.
- **Supported behavior** means deterministic public tests exercise that fact.
- **Experiment result** means a bounded evaluation informed a decision; its domain and fixture limits
  still apply.
- **Interpretation** explains why the result matters and must not be upgraded into a broader claim.
- **Limitation** marks behavior or evidence the repository does not provide.

The [research index](research/README.md) labels supported, rejected, inconclusive, and incomplete
findings. The [provenance record](provenance.md) explains why some private source hashes and task
labels appear without the raw private campaign machinery.
