# ADR 0005 — Order bounded candidates by available evidence

- Status: accepted
- Date: 2026-06-30

## Context

The Task 06 adversarial fixture reproduced two insertion-order failures in the Task 03 retrieval
policy. Equal lexical matches were selected by node ID, and outgoing relations were admitted by
target ID before the edge budget was applied. Consequently, an earlier low-importance lexical
collision could replace the intended seed, while an earlier low-weight edge could starve stronger
evidence. Both outcomes changed when a semantically equivalent fixture was inserted in a different
order.

## Decision

Candidate admission uses evidence already present in the v1 model:

1. lexical seeds rank by matched-term count, node importance, kind, text, then node ID;
2. outgoing relations rank by weight, target importance, target kind, target text, target ID, then
   relation name;
3. final equal-score results rank by target importance, kind, text, then node ID.

The score formula, traversal shape, hard budgets, and result path contract do not change. Text and
kind are deterministic fallbacks, not relevance claims. Node ID remains the final fallback for
genuinely indistinguishable stored evidence. Equal-weight alternative paths still use the frozen
Task 03 path tie-break and are reported as a residual insertion-sensitive ambiguity.

## Alternatives

- Keep ID-first admission: rejected because the adversarial regression showed useful evidence can
  be excluded solely by insertion order.
- Add a learned reranker, embeddings, or synthesis: rejected as outside Task 06 and unnecessary to
  use existing evidence.
- Order only by text: rejected because lexical order is deterministic but is not a quality signal.

## Consequences

Higher-importance lexical matches and higher-weight edges consume scarce budgets first. Fixtures
with distinct evidence are invariant under insertion permutation at the semantic-result level.
Exact evidence ties remain deterministic within one database but may vary semantically across
different ID assignments; Task 06 reports that limitation explicitly.
