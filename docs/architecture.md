# Architecture

Memory3D is a local, synchronous Rust workspace with one dependency direction:

```text
CLI / Python / MCP -> memory3d-core -> SQLite
```

`memory3d-core` contains domain types, validation, persistence, retrieval, explicit feedback,
experimental community organization, and the evidence envelope. It performs no network access and
depends on no async runtime. Adapters translate inputs and outputs; storage, retrieval, and evidence
policy remain in the core.

## Domain and persistence

Domain types are validated before persistence. One SQLite file stores schema metadata, nodes,
relations, normalized lexical terms, explicit feedback, community artifacts, and evidence-envelope
records. Multi-step writes and migrations are transactional.

Schema version 7 migrates supported historical versions. Opening a newer schema is refused without
altering the file. Node IDs remain stable across reopen, relation endpoints must exist, weights must
be finite and within their documented range, and optional coordinates must be finite and within
±1,000,000 in the caller's declared frame. SQLite is retained for the tested local, one-writer
workflow; no alternate backend is shipped.

## Plain retrieval pipeline

1. The query is normalized into distinct lexical terms.
2. Bounded lexical seed selection ranks candidates by matched-term count descending, importance
   descending, kind ascending, text ascending, and node ID ascending.
3. Bounded activation follows outgoing weighted relations under explicit hop, result,
   visited-node, and visited-edge limits. Adjacency admission uses relation weight descending,
   target importance descending, target kind ascending, target text ascending, target node ID
   ascending, and relation name ascending.
4. The best path to each activated node is retained and returned from its lexical seed.
5. Final activation results use this exact order:
   1. score descending;
   2. importance descending;
   3. kind ascending;
   4. text ascending;
   5. node ID ascending.

Scores express retrieval relevance under this algorithm, not confidence or truth. Stable node IDs
resolve only exact stored-evidence ties. Traversal reports consumed node and edge work, while the
adjacency diagnostic can separately expose stored candidate rows.

Explicit feedback is an opt-in core and CLI path. It derives a call-local effective relation weight
from recorded positive or negative events and never mutates the raw relation weight. Community
routing is experimental/core-only, is never selected by default, and is not a promoted adapter
workflow.

## Evidence pipeline

An evidence bundle is a versioned textual DTO. Preview validates and summarizes it without writes;
apply requires an actor and policy and commits the bundle transactionally. Identical idempotent
replays return the earlier result, while key reuse with a different canonical payload fails.
Dependency-aware rollback is a compensating transaction, not history rewriting.

Each evidence item is projected to an ordinary node for the same lexical and graph relevance
pipeline. Scoped context assembly then scans ranked activation candidates and applies the evidence
policy independently:

```text
query -> lexical seeds -> bounded graph activation -> ranked candidates
      -> evidence projection lookup -> exact scope/lifecycle/conflict/supersession policy
      -> admitted + excluded items + abstention + work report
```

Candidate scanning and admitted-result count are separate bounds. The package reports traversal
work, candidate work, ordinary candidates skipped, evidence candidates evaluated, byte use,
exclusions, abstention, and an explicit stop reason. Abstention only describes the bounded scan.

## Adapter boundary

- Plain memory is exposed through the core, CLI, Python, and local stdio MCP server.
- The evidence envelope is exposed through the core, CLI, and MCP server.
- Explicit feedback is exposed through the core and CLI.
- Experimental community routing is core-only.

Adapter conformance tests compare their results with the same core reports. The Python and MCP
layers do not add network retrieval, model execution, or an alternative storage policy. See
[capabilities](capabilities.md), [limitations](limitations.md), and the claim-level
[verification guide](verification-guide.md).
