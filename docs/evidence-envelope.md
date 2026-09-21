# Evidence envelope

The evidence envelope is an explicit, operator-applied layer over ordinary Memory3D nodes. It keeps
associative relevance separate from epistemic eligibility and does not assign a universal truth,
confidence, or authority score.

An `EvidenceBundle` is a versioned JSON document containing immutable evidence IDs, text and kind,
source and producer identities, exact scope selectors, lifecycle state, optional observation time,
derivation links, conflicts, supersessions, and decision provenance. Structural validation is
bounded before any write occurs.

Lifecycle values are `candidate`, `observed`, `corroborated`, `approved`, `contradicted`,
`superseded`, and `rejected`. The settled policy admits only exact-scope `observed`, `corroborated`,
and `approved` evidence that is neither superseded nor part of a visible unresolved conflict.

The workflow is:

1. preview a bundle without changing storage;
2. apply it transactionally with an actor, policy, and idempotency key;
3. retrieve stored evidence by immutable ID;
4. assemble scoped context through bounded lexical and graph activation;
5. roll back an applied bundle only when no later durable evidence depends on it.

The actor and policy fields are caller-supplied metadata. The core validates and records them; it
does not authenticate actors, evaluate identity, or enforce an external authorization policy.

Identical idempotent replays return the prior result. Reusing a key for a different canonical
payload fails. Rollback is a compensating transaction and cannot erase evidence referenced by a
later bundle.

Context assembly has two independent result-side bounds. `result_limit` controls admitted evidence
and defaults to 10. `candidate_scan_limit` controls ranked activation candidates inspected and
defaults to 100, with a maximum of 1,000. Graph activation has its own seed, hop, node, and edge
bounds. The package reports traversal and candidate work, ordinary nodes skipped, evidence
candidates evaluated, admitted and excluded items, byte use, and one explicit stop reason:
candidate stream exhausted, candidate scan limit reached, result limit reached, or byte limit
reached.

`abstained=true` means that no evidence was admitted from the bounded candidate scan. It does not
prove that admissible evidence is absent outside the graph-traversal or candidate-scan bounds.

The core performs no model execution, automatic extraction, lifecycle promotion, authority
ranking, background work, or silent apply. A private cross-domain evaluation labeled Task 28 did
not establish promotion, so the workflow remains a bounded explicit contract rather than a general
safety claim.
