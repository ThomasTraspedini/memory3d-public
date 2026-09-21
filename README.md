# Memory3D

Memory3D is a deterministic, local persistent-memory kernel exploring the separation between
**associative relevance** and **epistemic eligibility**. It stores plain-text memories and explicit
evidence in SQLite, retrieves related material through bounded lexical and graph work, reconstructs
the path to every activation result, and applies caller-declared scope and lifecycle rules before
evidence enters a context package. It is a kernel and research artifact—not a chatbot-memory
product, vector database, complete autonomous memory manager, voice companion, or claim to solve
general machine memory.

> This document is an index to evidence, not evidence itself. Public claims are linked to the
> implementation, deterministic tests, architecture decisions, reproducible demos, and known
> limitations.

## The central idea

Retrieving something as relevant does not imply that it is current, authoritative, or eligible to
enter context.

Memory3D therefore keeps two operations distinct:

1. **Associative retrieval** starts with deterministic lexical seeds, follows weighted directed
   relations within explicit work limits, and ranks results with their scores and paths.
2. **Epistemic filtering** examines the evidence candidates found by that retrieval and applies an
   explicit policy for exact scope, lifecycle, supersession, unresolved conflict, relevance, result
   count, and bytes.

An ordinary memory is a validated plain-text graph node. Evidence is an immutable assertion or
observation projected into that same graph for relevance, plus source, producer, exact scope,
lifecycle, derivation, conflict, supersession, and decision records. An **admitted** item passed the
requested policy and bounds; an **excluded** item remains visible with the reason it did not pass.

The name comes from the project's original exploration of persistent memory for an agent operating
in a physical/environmental context; spatial retrieval is not a supported capability in this public
release. The public smart-home fixture keeps that motivation concrete without turning this
repository into a smart-home product or a complete embodied-agent system.

## See the lifecycle in two processes

From the repository root, choose a database path that does not already exist and run:

```bash
cargo run -q -p memory3d-cli -- --db /tmp/memory3d-lifecycle.db demo lifecycle ingest
cargo run -q -p memory3d-cli -- --db /tmp/memory3d-lifecycle.db demo lifecycle recall
```

The first process writes a bedroom humidity graph and eight evidence items. The second reopens the
same SQLite file and prints three views:

- **Associative retrieval** reaches a room memory through a stored `connects_to` relation and shows
  its score, path, and traversal work.
- **Evidence admitted by the settled policy** includes `bedroom-calibrated-normal-v2`,
  `bedroom-monitor-v2`, and `bedroom-sensor-miscalibrated-v1`. It excludes
  `bedroom-inspect-v1` and `bedroom-high-v1` as superseded, and `nursery-high-v1` because its exact
  room scope does not match.
- **Unresolved conflict** keeps both ventilation observations visible, admits neither, and returns
  `abstained: true`.

The later calibrated observation supersedes the earlier high-humidity reading; the routine
monitoring recommendation supersedes the earlier inspection recommendation. Supersession does not
delete history. Likewise, a nursery observation can be relevant to a humidity query while remaining
ineligible for a bedroom-scoped context, and neither side of a declared conflict is chosen
automatically.

`abstained: true` means that **no evidence was admitted from the bounded candidate scan**. It is not
proof that admissible evidence cannot exist outside the traversal or candidate-scan bounds. The
report makes those bounds observable—for example, `candidate scan: 8/8 inspected`—alongside admitted
and excluded counts and the stop reason.

For the full walkthrough, stable JSON mode, expected output, and explicit non-claims, read the
[memory lifecycle demo](docs/memory-lifecycle.md).

## Architecture at a glance

```text
CLI ─────┐
Python ──┼──> memory3d-core ──> SQLite
MCP ─────┘
```

The synchronous Rust core owns domain validation, schema migrations, persistence, lexical seed
selection, bounded graph activation, deterministic ranking, path reconstruction, explicit feedback,
evidence projection, evidence-context policy, and bounded candidate scanning. Adapters translate
inputs and outputs; they do not reimplement storage or retrieval rules.

Activation result ordering is exactly:

1. score descending;
2. importance descending;
3. kind ascending;
4. text ascending;
5. node ID ascending.

Lexical seed and adjacency admission have their own evidence-first order before their limits are
applied; see [architecture](docs/architecture.md) and
[ADR 0005](docs/decisions/0005-order-bounded-candidates-by-evidence.md). Node ID is the final
fallback, so exact stored-evidence ties are deterministic within one database but can reflect ID
assignment across separately constructed databases.

## Supported surfaces

| Capability | Support level | Public surfaces |
| --- | --- | --- |
| Plain memory, links, lexical search, bounded activation | Supported/default | Core, CLI, Python, MCP |
| Evidence envelope and scoped context assembly | Supported opt-in | Core, CLI, MCP |
| Explicit relation feedback | Supported opt-in | Core, CLI |
| Community-routed activation | Experimental/core-only | Core |
| Deterministic fan-out synthesis | Rejected/not promoted | Research record and ADR only |
| Embeddings, pruning, spatial retrieval, autonomous extraction | Intentionally absent/deferred | None |

“Supported” means public code and deterministic tests exercise the stated behavior. It does not
mean suitability for every domain or deployment. Community routing is retained as an experimental
core mechanism and is never selected by default; it is deliberately not a foreground workflow.
The detailed boundary is in [capabilities](docs/capabilities.md).

## Engineering boundaries

Memory3D is local and synchronous. Its tested persistence model is one SQLite file with a
single-writer assumption. Default seed selection is lexical, and both graph traversal and evidence
candidate scanning are bounded; relevant material can therefore remain outside a particular run.

The core does not autonomously extract evidence, resolve entities, semantically deduplicate text,
infer truth or confidence, widen scope, resolve conflicts, or promote lifecycle states. It provides
no voice, UI, hosted service, or product layer. The evidence workflow enforces its explicit recorded
contract, not a general cross-domain safety property. A private cross-domain evaluation labeled
Task 28 did not establish promotion. See [limitations](docs/limitations.md) for the concrete list.

## Read deeper and verify

The intended reading path supports progressively deeper review:

1. [Memory lifecycle](docs/memory-lifecycle.md) — the concrete behavior in one two-process fixture.
2. [Architecture](docs/architecture.md) — boundaries, data flow, ranking, persistence, and adapters.
3. [Capabilities](docs/capabilities.md) and [limitations](docs/limitations.md) — what is supported,
   experimental, rejected, or absent.
4. [Verification guide](docs/verification-guide.md) — claim-to-code, test, ADR, evidence, and
   limitation mapping.
5. [Research index](docs/research/README.md), [decisions](docs/decisions/README.md), and
   [provenance](docs/provenance.md) — selected findings, rationale, and the curated public lineage.

Run the public quality gates with:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

The public package version is `0.1.1`; the durable database schema version is `7`. Memory3D is
licensed under the [MIT License](LICENSE).
