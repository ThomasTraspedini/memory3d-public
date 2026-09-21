# Memory3D

Memory3D is a local, deterministic memory kernel built around durable plain-text nodes, weighted
relations, bounded explainable retrieval, and an explicit evidence-envelope workflow.

The Rust core owns validation, SQLite persistence, migrations, lexical search, graph activation,
feedback, and evidence lifecycle operations. Thin CLI, Python, and stdio MCP adapters share the
same database and core semantics.

This is a curated public lineage reconstructed from a larger private research repository. It is
not the original private Git history. See [provenance](docs/provenance.md),
[capabilities](docs/capabilities.md), [limitations](docs/limitations.md), and the runnable
[memory lifecycle demo](docs/memory-lifecycle.md).

## Verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

The public package version is `0.1.1`; the durable database schema version is `7`.

Task 28 did not establish cross-domain promotion. The evidence workflow remains an explicit,
bounded contract rather than a general safety or cognition claim.

Licensed under the MIT License.
