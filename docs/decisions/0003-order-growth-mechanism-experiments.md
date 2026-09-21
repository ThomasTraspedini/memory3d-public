# ADR 0003 — Order growth-mechanism experiments by utility-density risk

- Status: accepted for mechanism order; roadmap sequencing superseded by ADR 0004
- Date: 2026-06-30

## Context

The Task 05 controlled fixture supported bounded work, resistance to disconnected irrelevant
growth, and enrichment from additional related evidence. It also exposed a limitation: nDCG rose
from `0.6021` to `1.0`, while utility density fell from `0.0669` to `0.0526` because the plain graph
needed ten nodes and nine edges to return all nine relevant facts. The fixture did not test noisy
connected hubs, deletion policies, communities, or coordinate semantics.

## Decision

Freeze bounded weighted activation as the comparison baseline and test candidate organization
mechanisms in this order:

1. synthesis/abstraction, because the first observed weakness is declining quality per unit of
   traversal work as related evidence expands;
2. pruning, using a fixture with connected low-value edges and an explicit retention/rollback
   policy;
3. multi-resolution communities, after pruning supplies a credible noisy-graph baseline;
4. 3D locality, last, because coordinates still lack assignment and distance semantics.

Each experiment requires its own ADR, deterministic fixture, comparison with the frozen Task 05
baseline, and rollback criterion before it can become default behavior.

## Alternatives

- Start with 3D locality: rejected because Task 05 produced no evidence that coordinates encode a
  useful neighborhood.
- Start with pruning: deferred until synthesis tests whether recurring related structure can
  increase utility density without discarding evidence.
- Add every mechanism together: rejected because its contribution and failure mode would not be
  attributable.

## Consequences

Task 05 does not implement any of these mechanisms. ADR 0004 subsequently inserts adversarial
retrieval validation, persistence hardening, and then synthesis into the numbered roadmap. Negative
synthesis evidence may promote pruning, but must be recorded rather than tuned away.
