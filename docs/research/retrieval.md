# Retrieval research record

Memory3D's plain retrieval baseline is bounded lexical seed selection followed by bounded weighted
graph activation. The public kernel exposes the consumed node and edge work and reconstructable
paths. This is a deterministic retrieval contract, not a claim of semantic understanding.

Adversarial fixtures in the private research lineage found that ID-first admission could let early,
weak candidates consume tight budgets. ADR 0005 therefore orders lexical seeds and graph edges by
available stored evidence before stable fallbacks. The same work found that connected low-value
edges and hubs can still reduce top-k quality even when traversal remains bounded.

A deterministic fan-out synthesis overlay was also tested. Across the recorded paired cases it
did not improve utility density, increased storage candidate rows, and usually reduced useful raw
coverage under the same traversal budget. ADR 0006 rejects that mechanism. The rejected result is
retained because reproducible negative findings constrain future design; it is not a shipped
capability.

The underlying experiments were run in the larger private research repository. This public record
keeps the decisions and outcome without publishing raw campaign infrastructure. Private source
commit `f86309b41a702fe4745f491158b34b907c047e53` records the synthesis evaluation milestone; the
hash is provenance only and does not imply Git ancestry.

No pruning, embedding, synthesis, or spatial-retrieval mechanism is included in this release.
