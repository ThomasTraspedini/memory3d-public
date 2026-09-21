# AGENTS.md

## Authority

Use `README.md`, then `docs/architecture.md`, `docs/capabilities.md`, and
`docs/limitations.md` as the public behavior hierarchy. ADRs and research records explain why the
current boundary exists; code and deterministic tests are the executable source of truth.

## Verification

Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D
warnings`, and `cargo test --workspace --all-features` before declaring a change complete. Run
adapter-specific tests when their setup is available. A future contributor verification guide will
live at `docs/verification.md`.

## Boundaries and claims

Supported and experimental behavior must remain clearly separated. Community routing is
experimental/core-only. Embeddings, pruning, and spatial retrieval are outside the first public
release. Claims must be backed by code and deterministic tests. Limitations, rejected findings,
and incomplete promotion evidence must not be removed or silently upgraded.
