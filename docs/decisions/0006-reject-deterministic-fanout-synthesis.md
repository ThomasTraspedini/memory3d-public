# ADR 0006 — Optimize adjacency lookup and reject deterministic fan-out synthesis

- Status: accepted
- Date: 2026-06-30

## Context

Task 08 must compare synthesis with a fair plain-graph baseline. The ADR 0005 adjacency query used
`relations_source_idx`, then a temporary B-tree to order every candidate by weight and target
evidence before `LIMIT`. On the degree ladder, its plan was `SEARCH r USING INDEX
relations_source_idx (source_id=?)` plus `USE TEMP B-TREE FOR ORDER BY`. Bounded activation counters
therefore did not imply bounded SQLite candidate work.

The tested synthesis candidate groups at least three outgoing relations with the same source,
relation name, and target kind. It adds one deterministic organization node, a `synthesizes` edge,
and one explicit `provenance:<raw relation>` edge per member. The node records the raw source,
relation, member count, and deterministic evidence fingerprint. Raw nodes and relations remain
unchanged in a disposable overlay.

## Decision

Schema v3 adds `relations_adjacency_idx(source_id, weight DESC)`, and the adjacency query explicitly
selects it. SQLite can consume the leading evidence order from the index, while retaining the
necessary temporary sort for target-importance, kind, text, ID, and relation-name tie-breaks. The
new read-only `inspect_adjacency` diagnostic reports the query plan, exact stored outgoing count,
returned row count, and observed query time. Activation reports adjacency source IDs so experiments
can sum stored candidate counts separately from admitted traversal edges. Ranking and traversal
semantics do not change.

Reject `deterministic-fanout-v1` as a production or opt-in core feature. Across 36 paired Task 06
cases it improved utility density zero times, worsened it 30 times, and was unchanged when no group
was eligible. Mean provenance-expanded Recall@9 fell from `0.7963` to `0.4630`, mean nDCG@9 from
`0.8516` to `0.6121`, and mean utility density from `0.0523` to `0.0450`. At degree 10,000 the
overlay increased candidate rows from 10,000 to 10,001, occupied about 6.03 MB versus 4.18 MB, and
took about 60 ms to build on the recorded host. Small latency differences are observational and do
not offset the quality and storage result.

The synthesis fixture remains benchmark/test code only. Disabling or rolling it back means
discarding the overlay and continuing with the unchanged raw database. Added or changed matching
evidence invalidates its fingerprint and is reported stale; no automatic rebuild runs.

Per ADR 0003, pruning becomes the next organization experiment after the MVP adapter/release
sequence. It is not added to Task 09 and does not change the current roadmap.

## Alternatives

- Promote synthesis because it reduces admitted nodes in selected cases: rejected because absolute
  quality fell and candidate storage work did not shrink.
- Remove raw fan-out edges inside synthesized mode: rejected because rollback would no longer be a
  lossless disable operation and the comparison would hide raw evidence.
- Drop evidence tie-breaks to avoid every temporary sort: rejected because it would undo ADR 0005
  semantics. The v3 index removes the avoidable leading-weight sort while preserving those rules.
- Keep the mechanism as a production opt-in: rejected because reproducibility and provenance are
  necessary but not sufficient evidence of utility.

## Consequences

Existing schema-v2 databases migrate transactionally to v3 by adding one index. Opening a newer
schema remains non-mutating. The index costs storage and write maintenance but supplies an honest,
regression-tested plain baseline. Synthesis causes no production API or default-behavior change.
Future organization experiments must compare against this schema-v3 baseline and retain the same
candidate-work accounting.
