# AGENTS.md

## Public source hierarchy

Use `README.md`, then `docs/architecture.md`, `docs/capabilities.md`, and `docs/limitations.md` as the
public document hierarchy. `docs/verification-guide.md` maps claims to implementation, tests,
decisions, demos, research records, and limits. Code and deterministic tests are executable
evidence; documents are indexes and explanations.

## Change verification

Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D
warnings`, and `cargo test --workspace --all-features`. Run an adapter or lifecycle demo when its
documented command or output changes.

## Claim boundaries

Keep supported, opt-in, experimental, rejected, and absent behavior distinct. Community routing is
experimental/core-only. Claims must remain backed by public code and tests. Rejected, inconclusive,
or incomplete findings must not silently become supported claims.
