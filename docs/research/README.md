# Selected research findings

This is a concise index of findings that changed an engineering decision. It is not a laboratory
notebook, and narrow experiments do not establish broad claims. Public code and tests verify the
released behavior; private-source findings are provenance summaries rather than independently
reproducible public benchmarks.

| Status | Finding | Engineering consequence | Public record |
| --- | --- | --- | --- |
| **Supported** | Under the tested local persistence requirements, SQLite conformance passed and no failing requirement justified replacement cost. | Retain SQLite; do not add an alternate backend for novelty. | [ADR 0015](../decisions/0015-freeze-storage-decision-gate.md) |
| **Supported** | Plain associative relevance was insufficient for one scoped context-assembly evaluation: a Kafka-only rule was returned for a filesystem request, with additional provenance, conflict, and supersession misses. | Keep relevance separate from explicit evidence eligibility. | [Failure record](epistemic-safety.md), [ADR 0021](../decisions/0021-adopt-bounded-evidence-envelope.md) |
| **Rejected** | Deterministic fan-out synthesis did not improve utility density in the recorded paired cases and usually reduced useful raw coverage. | Do not ship or promote the mechanism. | [Retrieval record](retrieval.md), [ADR 0006](../decisions/0006-reject-deterministic-fanout-synthesis.md) |
| **Rejected** | An apparently favorable result in the private cross-domain evaluation labeled Task 28 was invalidated when evaluator bias was detected. | Do not use the favorable result as promotion evidence. | [Limitations](../limitations.md) |
| **Inconclusive** | Corrected cross-domain evidence did not establish that the evidence workflow generalizes beyond its tested scenarios. | Keep claims scoped to the explicit implemented contract. | [Evidence envelope](../evidence-envelope.md) |
| **Incomplete** | Cross-domain promotion remains unearned; the public repository contains no reproducible evidence that closes that gate. | Describe Task 28 as incomplete for promotion, not as a passing or failing universal-safety result. | [Verification guide](../verification-guide.md) |

Negative and inconclusive results are part of the product boundary. They must not silently become
supported claims when documentation or adapters change.
