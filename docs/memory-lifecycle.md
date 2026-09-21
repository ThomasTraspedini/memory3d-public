# Memory lifecycle demo

This is the shortest concrete explanation of Memory3D. Two CLI processes use one SQLite file to
show durable ordinary memory, associative retrieval, evidence lifecycle, epistemic filtering, and
bounded abstention. The fixture is a presentation of public core behavior, not a separate inference
system.

## The two layers

**Ordinary memory** is a validated plain-text graph: nodes have kind, text, importance, metadata,
and optional caller-defined coordinates; weighted directed relations connect them. **Associative
retrieval** selects lexical seeds, traverses outgoing relations within hop, node, and edge budgets,
then returns scores and reconstructable paths.

**Evidence** is projected into that graph so it can be retrieved for relevance, but it also has an
immutable evidence ID, source and producer identities, exact scope, lifecycle, derivation,
conflicts, supersessions, and decision provenance. Applying a bundle requires an explicit actor and
policy.

**Epistemic filtering** evaluates retrieved evidence under caller-supplied rules. The settled policy
used here admits only exact-scope evidence in `observed`, `corroborated`, or `approved` lifecycle
states, and excludes evidence that is superseded or part of an unresolved conflict. Admitted and
excluded items are both reported. Relevance answers “what stored material is connected to this
query?”; eligibility answers “which retrieved evidence may enter this context under this policy?”

## The humidity scenario

The ingest process creates a small home-environment graph and applies three authorized bundles:

1. Sensor H-17 reports persistently high bedroom humidity (`bedroom-high-v1`), and an inspection is
   recommended (`bedroom-inspect-v1`).
2. A calibration finding records that H-17 was miscalibrated
   (`bedroom-sensor-miscalibrated-v1`). A calibrated instrument reports normal bedroom humidity
   (`bedroom-calibrated-normal-v2`), and routine monitoring (`bedroom-monitor-v2`) supersedes the
   inspection recommendation. A humidity observation from the nursery (`nursery-high-v1`) remains
   relevant but has a different exact scope.
3. Two bedroom ventilation observations (`bedroom-ventilation-open-v1` and
   `bedroom-ventilation-closed-v1`) explicitly conflict, with no automatic winner.

The recall process reopens the same file. Nothing is inferred about truth, entities, scope, or
lifecycle: the fixture supplies those records explicitly through the public core API.

## Run it

From the repository root, use a database path that does not already exist:

```bash
cargo run -q -p memory3d-cli -- --db /tmp/memory3d-lifecycle.db demo lifecycle ingest
cargo run -q -p memory3d-cli -- --db /tmp/memory3d-lifecycle.db demo lifecycle recall
```

The ingest command refuses to replace a populated database unless `--replace` is supplied. Add
`--json` before `demo` for stable machine-readable output.

The important part of the human-readable recall output is:

```text
SETTLED EVIDENCE
admitted:
  bedroom-calibrated-normal-v2 lifecycle=observed ...
  bedroom-monitor-v2 lifecycle=approved ...
  bedroom-sensor-miscalibrated-v1 lifecycle=approved ...
excluded:
  bedroom-inspect-v1 reason=superseded ...
  bedroom-high-v1 reason=superseded ...
  nursery-high-v1 reason=scope_mismatch ...
abstained: false
candidate scan: 8/8 inspected; evidence=6 ordinary_skipped=2; admitted=3/10; ...

UNRESOLVED CONFLICT
admitted:
  (none)
excluded:
  bedroom-ventilation-closed-v1 reason=conflict ...
  bedroom-ventilation-open-v1 reason=conflict ...
abstained: true
candidate scan: 4/4 inspected; evidence=2 ordinary_skipped=2; admitted=0/10; ...
```

Each printed item also includes its relevance score and path. Superseded evidence remains stored and
visible rather than being overwritten. Scope mismatch means the evidence selectors do not exactly
match the requested bedroom scope. Conflict means a visible competing item prevents admission; the
core does not decide which observation is correct.

Candidate scanning is a separate bound from graph traversal and from the admitted-result limit.
`candidate scan: 8/8 inspected` means the configured scan examined eight ranked activation
candidates in this fixture. `abstained: true` means **no evidence was admitted from that bounded
scan**. Neither statement proves that admissible evidence cannot exist outside the traversal or
candidate-scan bounds.

## What the demo establishes—and what it does not

The demo provides reproducible evidence for reopen persistence, stable evidence IDs, graph paths,
exact-scope exclusion, supersession visibility, conflict exclusion, candidate accounting, stop
reasons, and explicit abstention. Its end-to-end assertions live in
[`crates/memory3d-cli/tests/demo_e2e.rs`](../crates/memory3d-cli/tests/demo_e2e.rs).

It does not infer truth, confidence, authority, scope, entities, conflicts, or lifecycle
transitions. It does not use embeddings, an LLM, spatial retrieval, background consolidation,
autonomous ingestion, or automatic conflict resolution. It does not establish global absence or
domain-independent epistemic safety. See the [verification guide](verification-guide.md) to follow
each broader claim into code, tests, decisions, and known limits.
