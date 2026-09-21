# Architecture

Memory3D is an offline, single-writer Rust workspace with one dependency direction:

```text
CLI / Python / MCP -> memory3d-core -> SQLite
```

`memory3d-core` contains domain types, validation, persistence, retrieval, feedback, experimental
community organization, and the evidence envelope. It performs no network access and depends on
no async runtime. Adapters translate inputs and outputs but do not own retrieval or storage rules.

## Persistence

One SQLite file stores schema metadata, nodes, relations, normalized lexical terms, explicit
feedback, community artifacts, and evidence-envelope records. Schema version 7 migrates supported
historical versions transactionally. Newer schemas are refused without altering the file. IDs are
stable across reopen, relation endpoints require existing nodes, and caller-provided coordinates
must be finite and within ±1,000,000 in the caller's documented frame.

## Retrieval

Search normalizes and deduplicates lexical terms. Activation selects bounded lexical seeds and
traverses outgoing weighted relations under explicit hop, result, visited-node, and visited-edge
limits. Every result contains its score and a reconstructable path.

Candidate and result ordering is deterministic: score or matched-term evidence descending, then
importance descending, kind ascending, text ascending, and node ID ascending. Adjacency also uses
relation weight descending and relation name as its final fallback. Stable IDs resolve only exact
stored-evidence ties.

Feedback-aware activation is opt-in and never mutates raw relation weights. Community routing is
experimental/core-only; it is retained because schema and migration conformance include its
persisted artifacts, but it is not a default or foreground adapter capability.

## Evidence envelope

Evidence bundles add immutable IDs, identities, exact scope, lifecycle, derivation, conflicts,
supersession, and decision provenance. Preview, transactional apply, idempotent replay, guarded
rollback, and scoped context assembly are implemented in the core and exposed through CLI and MCP.
Relevance and epistemic state remain separate.

Context assembly independently bounds admitted results and ranked candidates scanned. It reports
consumed work, exclusions, abstention, byte use, and an explicit stop reason.
