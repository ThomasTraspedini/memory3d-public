# Provenance

This repository is a curated public lineage. It was deliberately reconstructed from a larger
private research repository, which remains the research authority.

Public commits represent coherent engineering milestones rather than the original private Git
history. The repositories share no Git ancestry: no private `.git` directory, refs, objects,
branches, tags, or remotes were copied into this repository.

Legacy code, internal task machinery, raw experimental infrastructure, model artifacts, campaign
material, evaluation constitutions, application/audit material, and unrelated internal files were
deliberately excluded. Their omission does not imply that they never existed.

Private source commit hashes may be referenced as provenance for a decision or result. Such a hash
identifies source evidence only and does not imply public ancestry.

The public projection preserves the corrected private behavior selected for release, including
schema 7, coordinate storage bounds, evidence scan/admission separation, explicit evidence stop
reasons, bounded abstention, and deterministic evidence-first tie-breaking. It intentionally omits
unpromoted embedding, pruning, and spatial-retrieval mechanisms.

The reconstructed milestones use current commit dates and ordinary SemVer. Public package version
0.1.0 is independent of private task and version numbering.
