# Control plane, runtime, and extensions

AgentMesh now exposes complete module boundaries for local, standalone, gateway-only,
control-plane-only, and all-in-one deployments. The implementation stays usable as one binary while
allowing the control and data planes to be deployed separately later.

## Desired state

`agentmesh-storage` stores tenant- and namespace-scoped JSON resources behind a backend-neutral
contract. Writes use create or revision-match conditions, documents have hard size limits, and list
operations are bounded. In-memory and SQLite implementations share the same contract suite.

`agentmesh-control-plane` validates desired resources, orders them canonically, excludes disabled
resources, and compiles immutable runtime snapshots. Every snapshot has a monotonic revision and a
SHA-256 integrity digest. Invalid revisions never replace the last active snapshot. Gateways report
accepted or rejected rollout state separately from desired state.

## Administrative API and CLI

`agentmesh-control-api` provides a versioned Axum router with:

- bearer authentication stored only as a SHA-256 digest;
- tenant-scoped resource application with optimistic concurrency;
- explicit reconciliation and active-snapshot retrieval;
- operational status and an SSE event bootstrap;
- shared disclosure-safe error envelopes.

The CLI supports configuration validation, JSON Schema generation, semantic diff output, local
diagnostics, resource apply, reconciliation, and snapshot inspection. Administrative tokens can be
provided through `AGENTMESH_ADMIN_TOKEN` without being printed by command diagnostics.

The local durable service is started with:

```bash
AGENTMESH_ADMIN_TOKEN='replace-with-at-least-16-bytes' \
  cargo run -p agentmesh -- control-plane --database agentmesh-control.db
```

## Runtime profiles

`agentmesh-runtime` validates a component dependency graph before startup. Components start in
topological order, a failed start rolls back already-started dependencies, readiness aggregates
component state, and shutdown always runs in reverse order. The profile vocabulary covers `dev`,
`standalone`, `gateway`, `control-plane`, and `all-in-one` composition.

## Extensions and testing

`agentmesh-plugins` begins with compile-time Rust implementations and a language-neutral JSON call
boundary. Plugins declare one extension point and their network, secret, configuration, or telemetry
permissions. Registration and every invocation fail closed unless the operator and request context
grant all requested permissions. Payloads are bounded so the contract can later back a WASM host.

`agentmesh-testkit` centralizes fake monotonic time, deterministic random selection, scripted MCP
messages, disconnects and classified failures. This makes timing, selection, cancellation, and
failure behavior reproducible across crates.

## Data-plane completion

The remaining foundational modules were extended together with the control plane:

- core identifiers and request context carry explicit tenant and configuration revision data;
- configuration supports deterministic YAML/environment precedence and atomic hot reload;
- protocol types cover tools, resources, prompts, tasks, and bounded cursors;
- transport adds session expiry/cancellation, WebSocket framing, and allowlisted child supervision;
- discovery provides DNS, container, Kubernetes, self-registration, and plugin adapters;
- gateway middleware has one validated fail-closed order from admission through accounting.

Production PostgreSQL, Redis, secret-manager, OpenTelemetry, dashboard, and Kubernetes adapters can
be added behind these stable contracts without changing the domain or hot-path APIs.
