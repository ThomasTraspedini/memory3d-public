# ADR 0002 — Test scale-aware associative growth as a foundational objective

- Status: accepted
- Date: 2026-06-30

## Context

The original Memory3D idea was not merely to store a graph. It aimed for a memory whose usefulness
increases as related knowledge accumulates, without query work growing proportionally with all
stored data. Relations, synthesis, distance, and 3D organization were candidate mechanisms for
that behavior.

The initial restart plan preserved multi-hop retrieval but deferred most growth-related work until
after adapters. That sequence risked producing a functional graph wrapper without testing the
project's foundational idea.

## Decision

Treat scale-aware associative enrichment as an explicit, falsifiable MVP hypothesis. After the
two-process Rust demo and before persistence hardening, Python, or MCP:

- impose and expose hard graph-traversal budgets;
- benchmark related and irrelevant corpus growth independently;
- compare lexical-only retrieval with bounded weighted activation;
- report quality, traversal work, database work, latency, and utility density;
- use the evidence to order synthesis, pruning/community, and spatial-locality experiments.

SQLite remains the first persistence backend. The custom intellectual property under test is the
graph organization and retrieval engine, not the database page format.

## Alternatives

- Test growth only after Python/MCP: rejected because adapters would solidify an API before the
  defining behavior was measured.
- Require synthesis and 3D retrieval in the first functional demo: rejected because their semantics
  and comparison baseline do not yet exist.
- Assume graphs scale better by construction: rejected because unbounded graph traversal can degrade
  severely with graph size and degree.

## Consequences

The roadmap gains Task 05 and adapter tasks move later. The project may discover negative or
inconclusive evidence; such outcomes are accepted and must guide the next mechanism rather than be
hidden. Synthesis and 3D locality remain foundational candidates with scheduled evidence gates, but
they cannot be advertised as working advantages until benchmark-backed ADRs support them.
