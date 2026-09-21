# Capabilities and support levels

Support labels in this repository describe public code and deterministic tests. They do not imply
general domain suitability or product completeness.

| Capability | Level | Core | CLI | Python | MCP |
| --- | --- | :---: | :---: | :---: | :---: |
| Plain-text nodes and weighted directed links | Supported/default | ✓ | ✓ | ✓ | ✓ |
| Node metadata and optional caller-defined coordinates | Supported/default | ✓ | — | ✓ | ✓ |
| Lexical search and bounded graph activation with scores, work counters, and paths | Supported/default | ✓ | ✓ | ✓ | ✓ |
| Stable IDs, schema-7 migrations, and nontruncating reopen | Supported/default | ✓ | ✓ | ✓ | ✓ |
| Explicit database integrity report | Supported/default | ✓ | ✓ | — | — |
| Evidence preview, apply, immutable lookup, scoped context, and guarded rollback | Supported opt-in | ✓ | ✓ | — | ✓ |
| Evidence conflict, supersession, provenance, exclusions, scan accounting, and abstention | Supported opt-in | ✓ | ✓ | — | ✓ |
| Explicit feedback events and feedback-aware activation | Supported opt-in | ✓ | ✓ | — | — |
| Community artifact construction and community-routed activation | Experimental/core-only | ✓ | — | — | — |

The evidence workflow is opt-in because callers author the bundle, actor/policy metadata, scope,
and context policy explicitly; the core records and validates that metadata but does not
authenticate actors or enforce an external authorization policy. Ordinary activation does not
silently admit retrieved text under an evidence policy. Feedback is also opt-in and affects only
effective weights for a requested activation.
Community routing remains experimental, is never the default, and has no promotion claim.

## Rejected or not promoted

- **Deterministic fan-out synthesis:** implemented and evaluated in the private research lineage,
  then rejected because the tested mechanism did not improve utility density and reduced useful
  raw coverage under the same traversal budget. It is not shipped as a production or opt-in API.
- **Cross-domain promotion of the evidence workflow:** not established. An apparently favorable
  private evaluation result was rejected after evaluator bias was detected; corrected evidence did
  not support promotion.

## Intentionally absent or deferred

No public surface provides embeddings, learned ranking, pruning, spatial retrieval, autonomous
evidence extraction, entity resolution, semantic deduplication, model execution, automatic truth or
confidence inference, automatic conflict resolution, background lifecycle changes, binary payloads,
a custom storage engine, or a voice/UI/product layer.

See [limitations](limitations.md) for operational consequences and the
[verification guide](verification-guide.md) for claim-level evidence.
