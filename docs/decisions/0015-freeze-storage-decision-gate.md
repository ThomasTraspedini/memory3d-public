# ADR 0015 — Retain SQLite after conformance testing

- Status: accepted
- Date: 2026-07-27

## Context

Memory3D uses a versioned, local SQLite database behind the core repository boundary. A private
conformance campaign measured a representative adapter workflow, repository and graph-engine time,
query plans, migrations, reopen behavior, transactional failures, and disconnected scale points.
The decision gate required both a failed product threshold and material storage dominance before an
alternate-backend experiment could begin.

## Decision

Retain SQLite. Repository conformance passed, every frozen product threshold passed, and the
expected lexical and adjacency indexes were used. Storage represented most measured core query
time, but dominance alone is not evidence that replacement would improve a failing requirement.

Any future backend must preserve schema refusal, atomic writes and migrations, constraints, stable
IDs, deterministic ordering, exact value round trips, bounded retrieval semantics, and reopen
behavior. It must be justified by a measured requirement, not by novelty.

## Consequences

SQLite remains the only persistence implementation. No repository trait, alternate backend,
custom mapped-file format, or cross-backend migration is shipped. The public test suite preserves
the behavioral conformance boundary; host timing observations from the private campaign are not
presented as universal performance claims.

Private source commit `e84b677f25ed86e557126a09412e10e75f9879e9` records the original decision
milestone. The hash is provenance only and does not imply public Git ancestry.
