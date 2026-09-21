# ADR 0021 — Adopt one bounded evidence-envelope workflow

- Status: accepted
- Date: 2026-08-26

## Context

An internal evaluation exposed a narrow failure: relevant evidence declared for one scope was
treated as applicable to a request in another. The same evaluation exposed missing conflict,
supersession, and provenance distinctions. Plain activation can rank relevant text and reconstruct
graph paths, but it cannot determine whether evidence is eligible under a caller's policy.
Relevance and epistemic eligibility must remain separate.

A follow-up evaluation of one structural coding fixture favored bounded explicit curation over
model-generated expansion. No external or generated-memory integration met its entry criteria.
This narrow experimental origin supports a small caller-authored envelope, not a model runtime,
external system, automatic extractor, or enlarged graph policy.

## Evidence mapping and entry decision

The decision admits exactly one manually or externally authored, caller-applied workflow. Each
primitive responds to an observed failure or a durable-write requirement:

| Primitive | Motivation | Required effect |
| --- | --- | --- |
| Immutable evidence ID and raw content | Auditability and bounded explicit curation | Later bundles cannot overwrite or erase the original assertion. |
| Separate source and producer identities | Provenance distinctions | A producer is recorded separately from the declared source of its input. |
| Declared scope and observation/effective time | Cross-scope admission and stale-policy failures | Missing or mismatching scope is excluded, never widened. |
| Explicit lifecycle state | Candidate, supersession, rejection, and weighting failures | Retrieval exposes caller-authored status independently of relevance score. |
| Derivation references | Unsupported-inference findings and the existing reconstructable-path contract | Derived evidence remains traceable to stored evidence. |
| Conflict group and competing-item references | Missing conflict status | All recorded sides remain visible without an automatic winner. |
| Supersession references | Missing supersession warnings | Old evidence remains stored and is not admitted under the configured policy. |
| Decision provenance | Missing decision and lifecycle provenance | State-changing decisions record the caller-supplied actor, policy, and support. |
| Preview, idempotency, transactional apply, and rollback | Reviewable transitions and durable-write invariants | No candidate is silently or partially applied. |
| Bounded context assembly with admitted/excluded accounting and abstention | Cross-scope admission and categorical misses | Evidence that violates the caller-supplied scope, lifecycle, conflict, or supersession policy is not admitted. |

## Decision

Add a versioned textual `EvidenceBundle` workflow to `memory3d-core` and expose thin CLI and MCP
translations. The durable schema stores bundles, immutable evidence items, scopes, derivations,
conflicts, supersession links, and decision provenance separately from ordinary memory nodes.

Core validation is structural and bounded. The caller supplies an explicit context policy with
accepted lifecycle states and exact scope selectors. Scope matching is exact key/value matching;
an item with no scope or a missing/mismatching requested selector is excluded. The core does not
authenticate actors or infer truth or authority. Actor, policy, and evidence metadata and content
are caller-authored; the core validates their structure and records them but does not establish
their factual correctness or enforce an external authorization policy. Context assembly may use
ordinary bounded activation for relevance, but it reports lifecycle, scope, conflicts,
supersession, provenance, path, selection reason, admitted/excluded counts, byte estimate, and
consumed candidate/traversal work.

Applying a bundle is a transaction after deterministic preview. Its idempotency key identifies
the exact canonical payload: an identical replay reports the prior result, while reuse with a
different payload fails. Rollback is an explicit compensating transaction that removes only the
selected applied bundle and its graph projection; it never mutates evidence from another bundle.
Bundles referenced by later durable evidence cannot be rolled back.

The workflow is operator-applied. External models may construct the textual DTO, but model
execution, automatic extraction, lifecycle promotion, truth scoring, background work, and silent
apply remain outside the core and every adapter.

## Alternatives

- Keep the current memory contract: rejected because the evaluation observed a severe cross-scope
  admission and categorical conflict, supersession, and provenance failures.
- Add status fields only to ordinary node metadata: rejected because metadata has no typed
  validation, immutable evidence identity, transactional bundle boundary, idempotency, or bounded
  context policy.
- Integrate an external graph-memory system or generated-memory extraction: rejected because no
  external integration met the evaluation's entry criteria, while small explicit curation required
  less total work in the tested fixture.
- Assign a confidence or authority score: rejected because the evaluation does not define a
  domain-independent truth scale and relevance scores must not become truth probabilities.
- Automatically choose the newest or highest-authority conflict member: rejected because time and
  origin alone cannot justify automatically selecting a domain outcome.

## Consequences

Schema and public API complexity increase, and callers must provide explicit scope and lifecycle
policy. In return, the observed cross-scope admission can be prevented structurally and audited
without hiding raw evidence. Existing memories and plain activation remain unchanged and
deterministic.

The implementation subsequently proved migrations, reopen, adapter parity, atomic failure,
idempotency, rollback, and the bounded parameter-selection regression. The later cross-domain
evaluation did not establish promotion, so this ADR makes no cross-domain generalization claim.
