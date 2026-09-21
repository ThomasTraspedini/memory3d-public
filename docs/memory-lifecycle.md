# Memory lifecycle demo

This two-process demo shows how Memory3D combines durable associative memory with an explicit
evidence policy. It is a presentation of existing version-1 evidence-envelope and graph-retrieval
semantics, not a separate inference system.

## Ordinary memory and evidence

Ordinary memories are plain-text graph nodes connected by weighted directed relations. Retrieval
selects lexical seeds, follows a bounded number of relations, and returns scores plus
reconstructable paths.

Evidence is also projected into that graph for relevance retrieval, but it carries additional
immutable records: source and producer identities, exact scope, lifecycle, conflicts,
supersessions, and decision provenance. Applying evidence requires an explicit actor and policy.

Relevance answers “what graph material is connected to this query?” Epistemic eligibility answers
“which relevant evidence may this caller admit under the requested scope and lifecycle policy?” A
highly relevant item can therefore remain excluded.

## Scenario

The ingest process creates a bedroom environmental-memory graph and applies three authorized
evidence bundles. Sensor H-17 first reports persistently high humidity, leading to an inspection
recommendation. A later calibration finding identifies H-17 as miscalibrated, a calibrated
instrument reports normal humidity, and routine monitoring supersedes the earlier inspection
recommendation. A relevant nursery observation has a different exact scope. Two bedroom
ventilation observations remain in an unresolved conflict.

The recall process reopens the same SQLite file. Its first query reaches a useful room memory by a
stored graph relation, admits the calibrated observation and current recommendation, and keeps the
superseded and scope-mismatched candidates visible with reasons. Its second query sees only the
conflicting ventilation evidence, excludes both sides, and explicitly abstains.

## Run it

From the repository root, use one database path for two separate CLI invocations:

```bash
cargo run -q -p memory3d-cli -- --db /tmp/memory3d-lifecycle.db demo lifecycle ingest
cargo run -q -p memory3d-cli -- --db /tmp/memory3d-lifecycle.db demo lifecycle recall
```

The ingest command refuses to replace a populated database unless `--replace` is supplied. The
default human output identifies the queries, admitted and excluded evidence, exclusion reasons,
paths, abstention, traversal work, candidate work, result limits, and stop reasons. Add `--json`
before `demo` for stable machine-readable output.

The candidate report is deliberately bounded. `candidate scan: 8/8 inspected` means that this
fixture's configured scan admitted at most eight ranked candidates; it is not a claim that no
other evidence exists globally. Traversal reports its independent visited-node and visited-edge
bounds for the same reason.

## What this does not demonstrate

The demo does not infer truth, confidence, scope, entities, conflicts, or lifecycle transitions.
It does not use embeddings, an LLM, spatial retrieval, background consolidation, autonomous
ingestion, or automatic resolution. It does not establish global absence or domain-independent
epistemic safety. Every evidence state, relationship, scope, authorization identity, and decision
record in the fixture is supplied explicitly through the public core API.
