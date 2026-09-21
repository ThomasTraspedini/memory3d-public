# ADR 0021 — Adopt one bounded evidence-envelope workflow

- Status: accepted for Task 27 implementation
- Date: 2026-08-26

## Context

Task 25 rejected H7 for the qualified Phi-4 executor. The existing memory contract returned a
Kafka-only rule as settled for a filesystem request and also missed conflict, supersession, and
provenance gates. Plain activation can rank relevant text and reconstruct graph paths, but it
cannot state whether evidence is applicable, current, contested, or merely proposed. Relevance and
epistemic status must remain separate.

Task 26 supports bounded explicit curation over model-generated expansion on one structural
coding holdout. Its Graphiti prerequisite gate failed and every external or generated-memory
integration gate stayed closed. It therefore justifies a small caller-authored envelope, not a
model runtime, external system, automatic extractor, or enlarged graph policy.

## Evidence mapping and entry decision

The Task 27 entry gate passes for exactly one manually or externally authored, operator-applied
workflow. Every accepted primitive maps to frozen evidence:

| Primitive | Frozen evidence | Required effect |
| --- | --- | --- |
| Immutable evidence ID and raw content | Task 25 false-authority auditability and Task 26 curation result | Later bundles cannot overwrite or erase the original assertion. |
| Separate source and producer identities | Task 25 provenance misses | A producer cannot silently become the authority for its input. |
| Declared scope and observation/effective time | Task 25 Kafka/filesystem severe event and stale-policy family | Missing or mismatching scope is excluded, never widened. |
| Explicit lifecycle state | Task 25 candidate, supersession, rejection, and false-weight families | Retrieval exposes status independently of relevance score. |
| Derivation references | Task 25 unsupported-inference family and existing reconstructable-path contract | Derived evidence remains traceable to stored evidence. |
| Conflict group and competing-item references | Task 25 conflict-status misses | All sides remain visible without an automatic winner. |
| Supersession references | Task 25 zero-of-three supersession warnings | Old evidence remains stored and is excluded from settled context. |
| Decision provenance | Task 25 authority and lifecycle failures | State-changing decisions name the responsible actor or policy and support. |
| Preview, idempotency, transactional apply, and rollback | Task 25 reviewable-transition requirement plus durable-write invariants | No candidate is silently or partially applied. |
| Bounded context assembly with admitted/excluded accounting and abstention | Task 25 severe scope failure and categorical misses | Inapplicable or unsafe evidence cannot appear as settled. |

No Task 26 external component passes its independent integration gate, so none enters this design.

## Decision

Add a versioned textual `EvidenceBundle` workflow to `memory3d-core` and expose thin CLI and MCP
translations. The durable schema stores bundles, immutable evidence items, scopes, derivations,
conflicts, supersession links, and decision provenance separately from ordinary memory nodes.

Core validation is structural and bounded. The caller supplies an explicit context policy with
accepted lifecycle states and exact scope selectors. Scope matching is exact key/value matching;
an item with no scope or a missing/mismatching requested selector is excluded. The core does not
rank source authority or infer truth. Context assembly may use ordinary bounded activation for
relevance, but it reports lifecycle, scope, conflicts, supersession, provenance, path, selection
reason, admitted/excluded counts, byte estimate, and consumed candidate/traversal work.

Applying a bundle is a transaction after deterministic preview. Its idempotency key identifies
the exact canonical payload: an identical replay reports the prior result, while reuse with a
different payload fails. Rollback is an explicit compensating transaction that removes only the
selected applied bundle and its graph projection; it never mutates evidence from another bundle.
Bundles referenced by later durable evidence cannot be rolled back.

The workflow is operator-applied. External models may construct the textual DTO, but model
execution, automatic extraction, lifecycle promotion, truth scoring, background work, and silent
apply remain outside the core and every adapter.

## Alternatives

- Keep the current memory contract: rejected because Task 25 observed a severe scope/action event
  and categorical conflict, supersession, and provenance failures.
- Add status fields only to ordinary node metadata: rejected because metadata has no typed
  validation, immutable evidence identity, transactional bundle boundary, idempotency, or safe
  context policy.
- Integrate Graphiti or generated-memory extraction: rejected because Task 26 passed no external
  integration gate and favored small explicit curation on total work.
- Assign a confidence or authority score: rejected because frozen evidence does not define a
  domain-independent truth scale and relevance scores must not become truth probabilities.
- Automatically choose the newest or highest-authority conflict member: rejected because time and
  origin alone cannot authorize a domain decision.

## Consequences

Schema and public API complexity increase, and callers must provide explicit scope and lifecycle
policy. In return, the exact Task 25 failure can be prevented structurally and audited without
hiding raw evidence. Existing memories and plain activation remain unchanged and deterministic.

The implementation subsequently proved migrations, reopen, adapter parity, atomic failure,
idempotency, rollback, and the bounded parameter-selection regression. The later cross-domain
evaluation did not establish promotion, so this ADR makes no H7 or cross-domain generalization
claim.
