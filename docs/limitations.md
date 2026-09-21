# Engineering boundaries and limitations

These are design and evidence boundaries for the public release, not implied future promises.

## Deployment boundary

- The core is local and synchronous. It does not perform network access or run a model.
- SQLite is the only backend. The tested operating model is one local writer; multi-writer service
  operation, encryption, distributed synchronization, and hosted durability are outside scope.
- The repository is a kernel and adapter set, with no voice, UI, chatbot, hosted service, or end-user
  product layer.

## Retrieval boundary

- Default seed selection is lexical. It does not provide embedding similarity, learned ranking, or
  semantic understanding.
- Hop, result, seed, visited-node, and visited-edge limits bound activation. Bounded work does not
  imply universal latency, constant-time seed lookup, or exhaustive retrieval.
- Connected low-value edges, hubs, competing seeds, or material outside the work budget can reduce
  top-k quality.
- Exact ties end with stable node IDs. Results are deterministic within a database, but ID assignment
  can affect exact ties across separately constructed databases.
- Coordinates are stored and validated in a caller-defined frame; no public spatial-retrieval
  semantics are implemented.
- Feedback is explicit and opt-in. It is not reinforcement learning or autonomous correction.
- Community routing is experimental/core-only and has no promotion claim.

## Evidence boundary

- Candidate scanning is bounded independently from graph traversal and admitted-result count.
  Abstention means no item was admitted from the bounded scan, not that admissible evidence is
  globally absent.
- Scope matching is exact and caller-authored. The core does not infer or widen scope.
- The core does not autonomously extract evidence, resolve entities, semantically deduplicate text,
  infer truth, confidence, or authority, promote lifecycle states, or resolve conflicts.
- The settled policy filters recorded states and relationships; it cannot determine whether
  caller-supplied metadata or evidence content is factually correct.
- The evidence envelope structurally prevents the recorded scope mismatch under its explicit
  contract. It does not establish a general, model-independent, or cross-domain safety property.
- The private cross-domain evaluation labeled Task 28 did not establish promotion. That finding is
  incomplete for promotion purposes and must not be presented as generalization evidence.

See [capabilities](capabilities.md) for surface-level support and the
[verification guide](verification-guide.md) for the code and tests behind each bounded claim.
