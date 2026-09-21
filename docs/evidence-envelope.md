# Evidence envelope

The evidence envelope is an explicit, operator-applied layer over ordinary Memory3D nodes. It
keeps relevance separate from epistemic status and does not assign a universal truth score.

An `EvidenceBundle` is a versioned JSON document containing immutable evidence IDs, text and kind,
source and producer identities, exact scope selectors, lifecycle state, optional observation time,
derivation links, conflicts, supersessions, and decision provenance. Structural validation is
bounded before any write occurs.

The lifecycle values are `candidate`, `observed`, `corroborated`, `approved`, `contradicted`,
`superseded`, and `rejected`. The settled context policy admits only observed, corroborated, and
approved evidence when its scope exactly matches the requested selectors and it is neither
superseded nor unresolved by conflict.

The workflow is:

1. preview a bundle without changing storage;
2. apply it transactionally with an actor, policy, and idempotency key;
3. retrieve stored evidence by immutable ID;
4. assemble scoped context through bounded lexical/graph activation;
5. roll back an applied bundle only when no later durable evidence depends on it.

Identical idempotent replays return the prior result. Reusing a key for a different canonical
payload fails. Rollback is a compensating transaction and cannot erase evidence referenced by a
later bundle.

Context assembly has two independent bounds. `result_limit` controls admitted evidence and
defaults to 10. `candidate_scan_limit` controls ranked candidates inspected and defaults to 100,
with a maximum of 1,000. The package reports candidate work, ordinary nodes skipped, evidence
candidates evaluated, traversal work, admitted and excluded items, byte use, and one explicit stop
reason: candidate stream exhausted, candidate scan limit reached, result limit reached, or byte
limit reached. An empty admitted set is reported as abstention.

The core performs no model execution, automatic extraction, lifecycle promotion, authority
ranking, background work, or silent apply. Task 28 did not establish cross-domain promotion, so
the workflow remains bounded to this explicit contract.
