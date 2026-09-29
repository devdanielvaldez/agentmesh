# Changelog

All notable changes to AgentMesh will be documented here. The project follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Completed the platform module map with tenant-scoped storage contracts, in-memory and SQLite
  backends, optimistic concurrency, bounded documents, stable pagination, and isolation tests.
- Desired-state control plane with deterministic reconciliation, immutable SHA-256 runtime
  snapshots, atomic activation, gateway acknowledgements, and rollout status tracking.
- Authenticated versioned control API for resource apply, reconciliation, snapshot retrieval,
  operational status, and SSE readiness events, plus CLI apply/reconcile/snapshot workflows.
- Profile-aware runtime assembly with dependency validation, deterministic startup, readiness,
  rollback, and reverse-order graceful shutdown.
- Capability-restricted plugin registry with explicit grants, extension-point metadata, bounded
  payloads, and a future-WASM-compatible JSON invocation boundary.
- Shared deterministic testkit with fake time, seeded selection, scripted MCP transport failures,
  protocol fixtures, and stable error assertions.
- Expanded core domain vocabulary, layered configuration/env precedence, atomic hot reload, JSON
  Schema output, typed MCP tool/resource/prompt/task payloads, and bounded list cursors.
- Session TTL/cancellation, WebSocket framing, supervised allowlisted stdio processes, DNS,
  container, Kubernetes, self-registration and plugin discovery providers, plus a canonical
  fail-closed gateway pipeline.
- CLI configuration schema, diff and doctor commands together with control-plane resource commands.
- Opaque secret references and a tenant-scoped credential broker with pluggable providers,
  deterministic selection, short-lived caching, rotation/revocation, zeroing, and redacted leases.
- Central SSRF and egress defenses with host/port allowlists, public-IP enforcement, DNS pinning,
  redirect-safe destinations, bounded JSON, safe headers/redaction, and SHA-256 integrity quarantine.
- Revision-, identity-, protocol-, and tenant-aware MCP caching with explicit resource-read opt-in,
  TTL/byte/entry bounds, LRU eviction, invalidation, redacted leases, and metrics.
- Vendor-neutral W3C trace propagation, allowlisted low-cardinality attributes, bounded metric
  aggregation, structured events, and dropped-series/event accounting.
- Redacted audit events and a bounded tamper-evident SHA-256 outbox with required/best-effort
  delivery, retries, acknowledgements, stable exporter errors, and chain verification.
- Tenant-safe long-running MCP task mappings with opaque IDs, backend affinity, owner authorization,
  optimistic revisions, validated lifecycle transitions, expiry, cleanup, and bounded events.
- Pluggable authentication with normalized principals, redacted credentials, SHA-256 API-key
  snapshots, constant-time matching, expiry/disable controls, assurance, and safe audit evidence.
- Revisioned RBAC with roles, scoped bindings, explicit-deny precedence, delegated-user constraints,
  explainable decisions, capability visibility, and a bounded tenant-safe decision cache.
- Compiled contextual policy with deterministic precedence, risk/time/environment/metadata matching,
  obligations, simulation, default deny, and a separation-of-duties approval workflow.
- Atomic tenant/principal/IP/service/capability rate, concurrency, and quota limits with token-bucket
  refill, revisioned keys, RAII leases, bounded cardinality, metrics, and idle cleanup.
- Deterministic tenant-scoped routing with compiled precedence, conflict detection, capability
  verification, endpoint-label selection, and explainable decisions.
- Round-robin, random, least-active, weighted, consistent-hash, EWMA, and adaptive endpoint
  balancing with cancellation-safe RAII leases and bounded metrics.
- Active/passive endpoint health state machines, deterministic probe jitter, stale-signal handling,
  administrative quarantine, recovery warm-up, and bounded transition events.
- Per-endpoint and optional per-capability circuit breakers with rolling failure windows, cooldowns,
  bounded half-open probes, fail-fast errors, and observable state transitions.
- Deadline budgets, idempotency-safe retry policies and budgets, exponential jittered backoff,
  priority backpressure queues, concurrency bulkheads, and graceful draining primitives.
- Bounded MCP capability discovery with pagination, deterministic SHA-256 integrity fingerprints,
  cache hints, safe change policy, registry reconciliation, and static endpoint providers.
- Tenant-scoped MCP service registry with capability and endpoint catalogs, monotonic revisions,
  optimistic concurrency, immutable snapshots, and interchangeable in-memory and SQLite backends.
- Pooled Streamable HTTP MCP proxy with static upstream configuration, request deadlines,
  bounded JSON and SSE responses, strict header allowlists, and isolated upstream credentials.
- Shared error taxonomy with stable codes, HTTP/MCP mappings, retry classification, and
  disclosure-safe public responses.
- Transport-independent MCP protocol module with JSON-RPC envelopes, modern and legacy version
  support, capability preservation, deterministic negotiation, and bounded message decoding.
- Object-safe MCP transport contract, bounded stdio and SSE framing, Streamable HTTP negotiation,
  and gateway admission validation for modern MCP requests.
- Initial Rust workspace with separate CLI, core, configuration, and gateway crates.
- Typed YAML configuration with strict validation.
- HTTP gateway with liveness, readiness, request IDs, tracing, compression, and panic recovery.
- Graceful shutdown and human-readable or JSON telemetry.
- Container, CI, security, contribution, and repository governance foundations.

[Unreleased]: https://github.com/devdanielvaldez/agentmesh/commits/main
