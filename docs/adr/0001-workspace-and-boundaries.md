# ADR 0001: Begin with a modular Rust workspace

- Status: Accepted
- Date: 2026-09-28

## Context

AgentMesh will grow into a control plane and a latency-sensitive data plane, but the first milestone
must remain easy to build, test, and run locally.

## Decision

Use a Rust workspace with independent crates for domain types, configuration, the HTTP gateway, and
the executable lifecycle. Keep deployment as one binary until operational needs justify additional
processes. Boundaries are expressed through crate APIs rather than a distributed architecture.

## Consequences

The initial system remains simple to operate while allowing components to evolve independently.
Cross-crate APIs require deliberate design, and a future control-plane split will need explicit
storage and configuration-distribution contracts.

