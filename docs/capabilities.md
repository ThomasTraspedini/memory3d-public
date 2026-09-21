# Capabilities

Supported public capabilities:

- durable plain-text memories, metadata, caller-provided coordinates, and weighted directed links;
- transactional batches, stable IDs, schema-7 migrations, integrity checks, and safe reopen;
- deterministic lexical search and bounded explainable graph activation;
- explicit opt-in relation feedback without raw-weight mutation;
- CLI, Python, and local stdio MCP adapters for plain memory;
- evidence preview, transactional apply, immutable lookup, scoped context assembly, idempotency,
  dependency-aware rollback, and CLI/MCP evidence operations;
- deterministic offline tests with no model or network dependency.

Experimental/core-only capability:

- deterministic community artifact construction and community-routed activation. It is retained to
  avoid schema and migration divergence, is never selected by default, and is not exposed as a
  promoted adapter workflow.

Not included in version 0.1.1: embeddings, pruning, spatial retrieval, synthesis, binary payloads,
automatic extraction, model execution, background lifecycle changes, or a custom storage engine.
