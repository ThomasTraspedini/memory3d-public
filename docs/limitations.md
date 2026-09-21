# Limitations

- Lexical retrieval does not provide semantic similarity, learned ranking, or truth assessment.
- Bounded graph work does not imply constant-time seed lookup or universal latency.
- Connected low-value edges, hubs, and competing seeds can reduce top-k quality within fixed work
  budgets.
- Exact evidence ties end with stable node IDs and can reflect insertion identity across separately
  constructed databases.
- Coordinates are stored and validated but have no public spatial-retrieval semantics.
- Feedback is explicit and opt-in; it is not reinforcement learning or automatic correction.
- Community routing is experimental/core-only and has no promotion claim.
- Evidence scope matching is exact and caller-defined. The core does not infer authority, resolve
  conflicts automatically, or convert relevance into confidence.
- The evidence workflow prevents the recorded scope failure structurally under its contract, but
  does not establish model-independent or cross-domain epistemic safety.
- Task 28 remains incomplete for promotion purposes: no cross-domain generalization is claimed.
- SQLite is evaluated for a local single-writer workflow; multi-writer service operation,
  encryption, distributed synchronization, and custom payload storage are outside this release.
