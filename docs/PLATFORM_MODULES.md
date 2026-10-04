# AgentMesh Platform Modules

> Internal module map for implementing AgentMesh as a local, cloud, and hybrid MCP platform.

**Status:** Working design  
**Scope:** Logical modules, Rust crate boundaries, ownership, dependencies, and delivery order  
**Publication:** This document is intentionally local and must not be committed or pushed yet.

## 1. Purpose

AgentMesh will grow from a single-process MCP gateway into a platform with an independently scalable
data plane and control plane. This document defines the modules needed to reach that architecture
without prematurely turning every boundary into a separate service.

The initial implementation should remain a Rust workspace and a single deployable binary. A module
becomes a separate process only when scaling, availability, security isolation, or ownership makes
that separation necessary.

## 2. Architectural planes

```text
                         Operators and automation
                                  │
                 CLI ─ Dashboard ─ API ─ GitOps controller
                                  │
                         ┌────────▼────────┐
                         │  Control plane  │
                         │ desired state   │
                         │ registry        │
                         │ policy/config   │
                         └────────┬────────┘
                                  │ versioned snapshots
                                  ▼
Clients ─ MCP protocol ──► ┌───────────────┐ ──► MCP servers
                           │  Data plane   │
                           │ auth, policy  │
                           │ route, proxy  │
                           │ resilience    │
                           └───────┬───────┘
                                   │
                            telemetry/audit
                                   ▼
                         Observability systems
```

The platform is divided into these areas:

1. Shared foundation and domain model.
2. MCP protocol and transport layer.
3. Data plane request processing.
4. Control plane and desired-state management.
5. Identity, security, and governance.
6. Reliability and traffic management.
7. Observability and auditing.
8. Persistence, caching, and coordination.
9. Operator, developer, and integration experience.
10. Deployment, extensions, and ecosystem.

## 3. Recommended workspace

```text
crates/
├── agentmesh/                    Main CLI and single-binary runtime
├── agentmesh-core/               Stable domain types and shared contracts
├── agentmesh-error/              Public error taxonomy and MCP error mapping
├── agentmesh-config/             File/env/remote configuration
├── agentmesh-protocol/           MCP parsing, validation, and negotiation
├── agentmesh-transport/          HTTP, SSE, WebSocket, and stdio transports
├── agentmesh-gateway/            Data-plane listener and request pipeline
├── agentmesh-proxy/              Streaming upstream/downstream forwarding
├── agentmesh-registry/           Servers, endpoints, tools, resources, prompts
├── agentmesh-discovery/          Static, DNS, container, and Kubernetes discovery
├── agentmesh-router/             Route matching and destination resolution
├── agentmesh-load-balancer/      Endpoint selection strategies
├── agentmesh-health/             Active and passive health management
├── agentmesh-circuit-breaker/    Per-endpoint circuit state machines
├── agentmesh-resilience/         Timeouts, retries, backpressure, and draining
├── agentmesh-authn/              Caller and workload authentication
├── agentmesh-authz/              RBAC and authorization decisions
├── agentmesh-policy/             General policy evaluation and approvals
├── agentmesh-rate-limit/         Rate, concurrency, and quota enforcement
├── agentmesh-credentials/        Credential references and brokering
├── agentmesh-security/           SSRF, egress, integrity, and payload defenses
├── agentmesh-tasks/              Long-running MCP task affinity and lifecycle
├── agentmesh-cache/              MCP-aware metadata and response caching
├── agentmesh-telemetry/          Metrics, traces, and structured logs
├── agentmesh-audit/              Durable security and governance events
├── agentmesh-storage/            Persistence interfaces and implementations
├── agentmesh-control-plane/      Desired state and configuration distribution
├── agentmesh-control-api/        Administrative REST/SSE API
├── agentmesh-runtime/            Component lifecycle and dependency assembly
├── agentmesh-plugins/            Native and future WASM extension contracts
└── agentmesh-testkit/            Test servers, fixtures, and conformance tools

dashboard/                        Operator web application
sdk/                              TypeScript, Python, and Rust SDKs
deployments/                      Docker, Compose, Kubernetes, and Helm
examples/                         Runnable local and cloud scenarios
benchmarks/                       Performance and regression suites
```

Not all crates should be created immediately. The catalog defines target boundaries; delivery order
is described in section 15.

### 3.1 Implementation status

This table tracks the implemented workspace boundaries. All 31 planned modules now have stable
contracts, bounded behavior, and test coverage; production infrastructure integrations remain
future adapters behind those contracts.

**Inventory:** 31 complete · 0 partial · 0 pending.

| Module | Status | Current scope / next gap |
| --- | --- | --- |
| `agentmesh` | Complete | Gateway lifecycle plus validate, schema, diff, doctor, apply, reconcile, and snapshot commands. |
| `agentmesh-core` | Complete | Validated scoped identifiers, request context, revisions, risk, server model, and clock contract. |
| `agentmesh-error` | Complete | Stable taxonomy, mappings, retry classes, and safe disclosure. |
| `agentmesh-config` | Complete | Strict YAML/environment precedence, validation, JSON Schema, secret references, and atomic snapshots. |
| `agentmesh-protocol` | Complete | JSON-RPC, negotiation, typed tool/resource/prompt/task payloads, and bounded validation/cursors. |
| `agentmesh-transport` | Complete | HTTP/SSE/stdio/WebSocket framing, bounded sessions, cancellation, and supervised child processes. |
| `agentmesh-gateway` | Complete | Operational HTTP surface, MCP proxying, and canonical fail-closed injectable request pipeline. |
| `agentmesh-proxy` | Complete | Bounded HTTP/SSE forwarding, credentials, timeouts, and response correlation. |
| `agentmesh-registry` | Complete | Catalog, bounded snapshots, optimistic revisions, in-memory and SQLite backends. |
| `agentmesh-discovery` | Complete | MCP catalogs plus static, DNS, container, Kubernetes, self-registration, and plugin providers. |
| `agentmesh-router` | Complete | Deterministic compiled routing, conflict detection, capability validation, and explanations. |
| `agentmesh-load-balancer` | Complete | Seven strategies, affinity, EWMA/adaptive scoring, RAII leases, and metrics. |
| `agentmesh-health` | Complete | Active/passive windows, state transitions, probe jitter, staleness, and event publication. |
| `agentmesh-circuit-breaker` | Complete | Per-endpoint/capability closed, open, and bounded half-open state machines. |
| `agentmesh-resilience` | Complete | Deadlines, safe retries/budgets, backpressure queues, bulkheads, and draining. |
| `agentmesh-authn` | Complete | Pluggable verifier chain, normalized principals, and secure API-key snapshots. |
| `agentmesh-authz` | Complete | Revisioned RBAC, explicit deny, inheritance, delegation, visibility, cache, and explanations. |
| `agentmesh-policy` | Complete | Compiled contextual rules, obligations, simulation, default deny, and approvals. |
| `agentmesh-rate-limit` | Complete | Scoped token buckets, concurrency, quotas, RAII leases, metrics, and bounded cleanup. |
| `agentmesh-credentials` | Complete | Opaque references, providers, selection, expiring cache, rotation, revocation, and redaction. |
| `agentmesh-security` | Complete | SSRF/egress, DNS pinning, payload/header bounds, redaction, integrity, and quarantine decisions. |
| `agentmesh-tasks` | Complete | Tenant-safe ownership, backend affinity, lifecycle revisions, expiry, cleanup, and events. |
| `agentmesh-cache` | Complete | Revision/identity-aware bounded cache, opt-in resources, LRU, invalidation, and metrics. |
| `agentmesh-telemetry` | Complete | W3C context, safe attributes, bounded metric series/events, and exporter snapshots. |
| `agentmesh-audit` | Complete | Redacted events, hash-chained outbox, delivery policies, retries, acknowledgements, and verification. |
| `agentmesh-storage` | Complete | Tenant-scoped documents, optimistic concurrency, bounds, and in-memory/SQLite backends. |
| `agentmesh-control-plane` | Complete | Desired-state reconciliation, integrity snapshots, atomic activation, and rollout tracking. |
| `agentmesh-control-api` | Complete | Authenticated versioned resource, reconcile, snapshot, status, and SSE endpoints. |
| `agentmesh-runtime` | Complete | Five profiles, dependency ordering, readiness, rollback, and reverse shutdown. |
| `agentmesh-plugins` | Complete | Permissioned compile-time extensions with bounded future-WASM invocation boundary. |
| `agentmesh-testkit` | Complete | Fake time, seeded selection, scripted MCP transport, fault injection, and assertions. |

## 4. Shared foundation

### 4.1 `agentmesh-core`

Owns the stable vocabulary used throughout the platform. It must not depend on networking,
databases, a web framework, or a specific policy engine.

Submodules:

- `identity`: `OrganizationId`, `NamespaceId`, `PrincipalId`, `AgentId`, `UserId`.
- `server`: `ServerId`, `ServiceId`, `EndpointId`, server metadata and lifecycle state.
- `capability`: tools, resources, prompts, tasks, and capability identifiers.
- `routing`: logical destinations, route references, weights, versions, and regions.
- `health`: health states, observations, reasons, and timestamps.
- `policy`: subjects, actions, resources, decisions, risk, and approval requirements.
- `request`: normalized request context and correlation identifiers.
- `config`: version/revision identifiers and desired-state metadata.
- `time`: injectable clock contracts for deterministic testing.

Rules:

- Prefer newtypes over raw strings for identifiers.
- Domain objects must be serializable but transport-neutral.
- Secrets and raw tokens must never implement `Debug` or `Display`.
- Tenant and namespace context must be explicit in every scoped identifier.

### 4.2 `agentmesh-error`

Defines one error taxonomy shared across transports and APIs.

Error families:

- protocol and schema errors;
- authentication and authorization failures;
- policy and approval outcomes;
- discovery and routing failures;
- upstream availability and timeout failures;
- rate, quota, and concurrency limits;
- storage and configuration errors;
- internal failures with opaque public messages.

Each error carries a stable code, safe client message, retryability, HTTP/MCP mapping, trace ID, and
optional internal cause. Sensitive causes are logged securely and never copied into public responses.

### 4.3 `agentmesh-config`

Loads bootstrap configuration from YAML, environment variables, and CLI flags. Later it also consumes
signed configuration snapshots from the control plane.

Responsibilities:

- strict schema and semantic validation;
- defaults and precedence rules;
- secret-reference parsing without secret resolution;
- deprecation warnings and configuration migrations;
- atomic hot reload;
- redacted diagnostic output;
- JSON Schema generation for editor and CI integration.

Configuration is separated into bootstrap settings, which require a process restart, and dynamic
settings, which can be swapped atomically.

## 5. MCP protocol and transport

### 5.1 `agentmesh-protocol`

Understands MCP semantics without performing network I/O.

Submodules:

- `message`: requests, notifications, responses, IDs, and JSON-RPC envelopes.
- `method`: known MCP methods and extension method handling.
- `capabilities`: client/server capability negotiation.
- `tools`, `resources`, `prompts`, `tasks`: typed MCP domain messages.
- `version`: supported versions and compatibility matrix.
- `validation`: bounded JSON/schema validation.
- `normalization`: canonical names, schemas, and hashes.
- `extensions`: registered protocol extension points.

Security constraints include maximum message size, nesting depth, schema complexity, tool count, and
parsing time. External schema references are disabled by default.

### 5.2 `agentmesh-transport`

Implements connections while keeping MCP message semantics in `agentmesh-protocol`.

Adapters:

- Streamable HTTP server and client.
- Server-Sent Events compatibility adapter.
- WebSocket adapter where supported.
- Local stdio client and supervised child process.
- Optional Unix domain socket transport for local deployments.

The transport layer owns connection limits, streaming bodies, cancellation, keepalive behavior,
session identifiers, and graceful connection draining. It does not make routing or policy decisions.

### 5.3 Protocol compatibility

A compatibility registry declares which transformations are safe between client and upstream MCP
versions. Negotiation results are stored per session and upstream endpoint. Unsupported version
combinations fail explicitly rather than silently dropping fields.

## 6. Data plane

### 6.1 `agentmesh-gateway`

Owns listeners and orchestrates the request pipeline. It should contain minimal business logic.

Pipeline stages:

```text
admission → protocol validation → authentication → context resolution
→ authorization/policy → limits → capability resolution → route selection
→ endpoint selection → resilience guard → proxy → response validation
→ accounting/audit/telemetry
```

Gateway submodules:

- public MCP endpoint;
- role-based virtual MCP endpoints;
- health and readiness endpoints;
- metrics endpoint;
- request context construction;
- middleware ordering;
- graceful shutdown and draining;
- readiness dependencies and degraded-mode behavior.

### 6.2 `agentmesh-proxy`

Forwards requests and streaming responses without unnecessary buffering.

Responsibilities:

- upstream request construction;
- safe header allowlisting and rewriting;
- request/response streaming;
- cancellation propagation;
- task/session affinity metadata;
- response validation and size limits;
- latency/error observation callbacks;
- connection pool management.

It must never forward caller credentials unless a credential policy explicitly permits a particular
token exchange or passthrough flow.

### 6.3 Request context

Every request receives an immutable context containing:

- request, trace, and session IDs;
- organization and namespace;
- authenticated principal and delegated user, when present;
- environment, region, and data classification;
- MCP version, method, and capability name;
- deadline, priority, and idempotency metadata;
- selected virtual server;
- configuration revision used for the decision.

The request context is passed explicitly. Tenant or identity state must not be hidden in process
globals or thread-local variables.

## 7. Registry and discovery

### 7.1 `agentmesh-registry`

Maintains the authoritative logical catalog of services and MCP capabilities.

Catalogs:

- logical MCP services;
- physical endpoints and replicas;
- tools and versions;
- resources and resource templates;
- prompts;
- long-running task handlers;
- virtual MCP servers;
- ownership, risk, environment, and lifecycle metadata.

Operations include register, update, drain, quarantine, disable, inspect, watch, and resolve. Every
mutation is tenant-scoped, versioned, validated, and auditable.

### 7.2 Capability discovery

Connects to an upstream server, negotiates MCP, lists capabilities, normalizes metadata, calculates
integrity hashes, and compares the result with the previous catalog revision.

Changes can be automatically accepted, rejected, or quarantined based on policy. Production defaults
should quarantine unexpected destructive capabilities or incompatible schema changes.

### 7.3 `agentmesh-discovery`

Discovers endpoints; it does not decide whether discovered endpoints are trusted.

Providers:

- static configuration;
- self-registration API with authenticated server identity;
- DNS and DNS SRV;
- Docker/container labels;
- Kubernetes Services, Endpoints, annotations, and future CRDs;
- plugin-provided service catalogs.

Discovery output is reconciled into desired/observed registry state. Identity verification and policy
determine whether an endpoint becomes routable.

## 8. Routing and traffic management

### 8.1 `agentmesh-router`

Produces a deterministic `RouteDecision` from a normalized request context and an immutable routing
snapshot.

Match dimensions:

- MCP method and capability name;
- virtual server;
- organization, namespace, principal, or role;
- environment and region;
- server version and tags;
- risk and data classification;
- time windows and feature flags.

Outputs include logical service, candidate pool, traffic policy, credential policy, selected route,
and a human-readable explanation. Route precedence must be deterministic and validated for conflicts.

### 8.2 `agentmesh-load-balancer`

Selects one eligible endpoint after routing and health filtering.

Strategies:

- round robin;
- random;
- least active requests;
- weighted selection;
- consistent hash for affinity;
- EWMA least latency;
- adaptive scoring using latency, errors, and load.

Endpoint accounting uses RAII-style leases so active-request counters are released on success,
failure, cancellation, or panic.

### 8.3 Weighted, canary, and regional routing

Traffic splitting is stable for a chosen affinity key to avoid moving a session between versions.
Canary health is evaluated independently. Regional routing respects data residency before latency;
policy constraints always take precedence over performance optimization.

## 9. Reliability

### 9.1 `agentmesh-health`

Combines active probes and passive request observations into endpoint health.

Components:

- probe scheduler with jitter;
- protocol-aware probe executor;
- rolling success, error, and latency windows;
- health transition state machine;
- outlier detection and ejection;
- recovery and warm-up behavior;
- health event publication.

An endpoint can be healthy, degraded, unhealthy, draining, disabled, or quarantined. Security
quarantine is distinct from operational unhealthiness.

### 9.2 `agentmesh-circuit-breaker`

Maintains independent closed, open, and half-open state per endpoint and optionally per capability.
State transitions use bounded windows and monotonic time. Cluster deployments share important state
or tolerate bounded local divergence by design.

### 9.3 `agentmesh-resilience`

Owns cross-cutting traffic protections:

- connect, request, idle, and total deadlines;
- retry classification and exponential backoff with jitter;
- retry budgets;
- idempotency enforcement for mutating tools;
- concurrency pools and bounded queues;
- load shedding and backpressure;
- priority classes;
- graceful endpoint and gateway draining;
- bulkheads per tenant, service, or tool.

Retries are disabled for non-idempotent calls unless the request has a verified idempotency mechanism.

### 9.4 `agentmesh-tasks`

Tracks long-running MCP tasks and preserves affinity to the backend that owns them.

It stores task ID mappings, owner principal, backend endpoint, status, expiry, cancellation state, and
trace references. Access to get, update, and cancel a task is authorized independently.

## 10. Identity, security, and governance

### 10.1 `agentmesh-authn`

Authenticates callers and workloads through pluggable verifiers:

- API keys with hashed storage and scoped metadata;
- JWT/JWKS validation;
- OIDC identities;
- OAuth access tokens;
- mTLS client certificates;
- workload identity and future SPIFFE integration;
- internal service accounts.

The output is a normalized principal plus authentication strength and evidence. Authentication never
implies authorization.

### 10.2 `agentmesh-authz`

Provides fast authorization for principal/action/resource tuples.

Capabilities:

- roles and role bindings;
- explicit allow and deny rules;
- tenant and namespace inheritance;
- tool visibility decisions;
- delegated-user constraints;
- decision caching tied to policy revisions;
- explainable matched-rule output.

Explicit deny wins. Cross-tenant access is rejected before resource lookup whenever possible.

### 10.3 `agentmesh-policy`

Evaluates richer contextual rules that go beyond RBAC.

Inputs can include identity, capability risk, environment, data classification, time, destination,
arguments metadata, and external approval state. Decisions are `allow`, `deny`, or
`require_approval`, with stable reason codes and obligations.

Submodules:

- policy compiler and validator;
- immutable evaluation bundle;
- decision engine;
- human approval workflow;
- policy simulation and dry-run mode;
- route/policy explanation;
- conflict analysis.

The hot path evaluates compiled policies and performs no database queries.

### 10.4 `agentmesh-rate-limit`

Enforces:

- token-bucket or sliding-window request rates;
- concurrent request limits;
- daily/monthly request quotas;
- future cost or token budgets;
- tenant, principal, IP, service, and capability scopes.

Single-node mode uses local state. Cluster mode uses Redis or another atomic shared backend where
strict global enforcement is required.

### 10.5 `agentmesh-credentials`

Separates agent credentials, AgentMesh identity, MCP server credentials, and downstream API secrets.

Components:

- opaque secret references;
- secret-provider interface;
- short-lived credential cache;
- credential selection policy;
- OAuth/token exchange broker;
- rotation and revocation hooks;
- redaction and exposure controls.

Provider implementations may support environment variables for development, Kubernetes Secrets,
Vault, and cloud secret managers. Persistent state stores references, not plaintext values.

### 10.6 `agentmesh-security`

Centralizes reusable defenses:

- SSRF-safe URL resolution and redirect handling;
- IP range and cloud metadata blocking;
- DNS rebinding defenses;
- upstream egress allowlists;
- request and response size bounds;
- schema complexity limits;
- sensitive header stripping;
- tool definition hashing and pinning;
- endpoint identity and certificate verification;
- quarantine decisions;
- secure redaction primitives.

Security primitives must be used by discovery, proxying, OAuth, health checks, and plugin networking;
each component must not invent its own URL safety behavior.

## 11. Observability and audit

### 11.1 `agentmesh-telemetry`

Provides consistent instrumentation without coupling domain crates to a vendor.

Outputs:

- structured logs with request and tenant-safe context;
- OpenTelemetry traces and W3C trace propagation;
- Prometheus/OpenTelemetry metrics;
- RED metrics per service and capability;
- health, policy, limit, cache, and circuit-breaker metrics;
- bounded-cardinality attribute rules.

Tool arguments, outputs, credentials, and personally identifiable data are excluded by default.

### 11.2 `agentmesh-audit`

Produces durable records for security-sensitive actions and control-plane changes.

Events include authentication, policy decisions, approvals, tool calls, credential access, registry
changes, configuration revisions, quarantines, and administrative actions.

Each event carries actor, tenant, action, target, outcome, policy/config revision, trace ID, timestamp,
and a redacted metadata envelope. Sinks include PostgreSQL, object storage, SIEM/webhook exporters, and
future immutable/WORM storage.

Audit delivery uses a bounded durable outbox in production. Failure policy is configurable: certain
high-risk operations may fail closed when audit durability cannot be guaranteed.

## 12. Persistence, cache, and coordination

### 12.1 `agentmesh-storage`

Defines repository traits and transaction boundaries for:

- organizations, namespaces, users, and principals;
- servers, endpoints, and capability catalogs;
- routes, policies, roles, and bindings;
- configuration revisions and rollout status;
- tasks and approvals;
- credential references;
- audit events and outbox entries.

Implementations:

- in-memory for unit tests;
- SQLite for local single-binary mode;
- PostgreSQL for cloud and highly available deployments.

Database-specific types must not leak into domain or data-plane interfaces.

### 12.2 `agentmesh-cache`

Provides revision-aware caches for tool catalogs, capability lists, discovery results, policy
decisions, JWKS documents, and explicitly cacheable resource reads.

It never caches `tools/call` by default. Cache keys include tenant, namespace, identity/policy scope,
protocol version, and configuration revision where relevant.

### 12.3 Coordination and events

Redis is optional for distributed rate limits, ephemeral coordination, and selected shared states.
NATS or another event bus is introduced only when independent processes require durable or scalable
event fan-out.

Representative events:

- `server.registered`, `server.health_changed`, `server.quarantined`;
- `capability.changed`;
- `route.updated`, `policy.updated`, `config.activated`;
- `approval.requested`, `approval.resolved`;
- `credential.rotated`.

Events use versioned envelopes, tenant IDs, idempotency IDs, timestamps, and trace correlation.

## 13. Control plane

### 13.1 `agentmesh-control-plane`

Owns desired state and converts persisted resources into validated, immutable runtime snapshots.

Controllers:

- server and endpoint reconciliation;
- capability discovery reconciliation;
- route compilation;
- policy compilation;
- identity and role reconciliation;
- configuration snapshot generation;
- gateway membership and rollout tracking;
- certificate and credential-reference coordination;
- approval lifecycle;
- garbage collection of expired state.

The control plane never sits on the normal MCP request path.

### 13.2 Configuration distribution

Gateways receive signed, versioned snapshots or deltas. They validate the artifact, stage it, and
atomically swap active state. If a new revision is invalid, the previous revision remains active and
the gateway reports rejection details.

A snapshot contains only hot-path data:

- services and endpoints;
- normalized capabilities and virtual catalogs;
- compiled routes and policies;
- identity verification settings;
- limits, resilience settings, and credential references;
- telemetry sampling and redaction rules.

### 13.3 `agentmesh-control-api`

Administrative API domains:

- `/servers`, `/endpoints`, `/capabilities`;
- `/routes`, `/virtual-servers`;
- `/policies`, `/roles`, `/bindings`, `/approvals`;
- `/principals`, `/api-keys`;
- `/tasks`, `/audit`, `/events`;
- `/config/revisions`, `/rollouts`;
- `/status`, `/diagnostics`.

The API provides pagination, filtering, optimistic concurrency, idempotency keys, structured errors,
fine-grained authorization, and SSE event streams. OpenAPI is generated and versioned.

## 14. Product and ecosystem modules

### 14.1 `agentmesh-runtime`

Assembles implementations, starts components in dependency order, reports readiness, and shuts down
in reverse order. It supports these profiles:

- `dev`: in-memory/SQLite, human logs, local stdio processes;
- `standalone`: one binary with embedded control and data planes;
- `gateway`: data plane connected to an external control plane;
- `control-plane`: API, controllers, and persistence without MCP traffic;
- `all-in-one`: small production or evaluation deployment.

### 14.2 CLI

The `agentmesh` binary eventually exposes:

- `serve`, `gateway`, and `control-plane`;
- `init`, `dev`, and `add`;
- `get`, `describe`, `apply`, `delete`, and `watch`;
- `validate`, `diff`, and `dry-run`;
- `test server` and `doctor`;
- `explain route` and `explain policy`;
- `traffic`, `logs`, and `tasks`;
- `config history` and `config rollback`.

Commands operate through the control API except bootstrap and local development commands.

### 14.3 Dashboard

The web application consumes only the control API and event stream. Views include overview, topology,
servers, capabilities, traffic, latency/errors, traces, policies, approvals, security events, audit,
configuration revisions, and cluster health.

The dashboard is not required for gateway operation and contains no privileged business logic.

### 14.4 SDKs

- Rust SDK for embedding/testing and control API access.
- TypeScript SDK for dashboard and automation.
- Python SDK for platform automation and agent ecosystems.

SDKs are generated from stable API specifications where possible. MCP clients do not need an
AgentMesh-specific SDK; they connect through standard MCP.

### 14.5 `agentmesh-plugins`

Extension points:

- authentication verifier;
- discovery provider;
- policy function;
- secret provider;
- telemetry/audit exporter;
- load-balancing strategy;
- request/response filter.

Begin with compile-time Rust traits. Dynamic native libraries are avoided. A future WASM host can
provide language-neutral, capability-restricted plugins with explicit CPU, memory, time, network,
and secret permissions.

### 14.6 `agentmesh-testkit`

Provides reusable testing infrastructure:

- deterministic mock MCP servers;
- malformed and adversarial message fixtures;
- latency, failure, and disconnect injection;
- fake clock and seeded random selection;
- policy and routing assertion helpers;
- protocol conformance cases;
- ephemeral SQLite/PostgreSQL helpers;
- end-to-end topology builder.

### 14.7 Benchmarks

Benchmark suites measure parsing, route matching, policy evaluation, load balancing, proxy overhead,
catalog scale, rate limiting, and end-to-end latency. Results compare direct MCP calls with proxied
calls and track P50/P95/P99 latency, throughput, CPU, memory, and allocation behavior.

## 15. Delivery order

### Phase 0 — Foundation (current)

- `agentmesh`, `agentmesh-core`, `agentmesh-config`, and `agentmesh-gateway`.
- Typed errors, runtime assembly, testkit basics, and CI quality gates.

### Phase 1 — Functional MCP proxy

- `agentmesh-error`.
- `agentmesh-protocol`.
- HTTP portion of `agentmesh-transport`.
- `agentmesh-proxy`.
- Static upstream configuration.
- Request limits, cancellation, tracing, and end-to-end tests.

Exit criterion: one client calls one upstream MCP server through AgentMesh with streaming and protocol
negotiation preserved.

### Phase 2 — Registry and deterministic routing

- `agentmesh-registry` with in-memory and SQLite storage.
- Capability discovery and catalog.
- `agentmesh-router`.
- `agentmesh-load-balancer` with round robin and least active requests.
- CLI server inspection commands.

Exit criterion: capability calls route correctly across two logical services and multiple replicas.

### Phase 3 — Reliability

- `agentmesh-health`.
- `agentmesh-circuit-breaker`.
- `agentmesh-resilience`.
- Task affinity.
- Failure-injection test suite.

Exit criterion: traffic automatically moves away from failed replicas without client reconfiguration
and without unsafe retries.

### Phase 4 — Security and governance

- API key and JWT authentication.
- RBAC authorization and tool visibility.
- Policy engine, rate/concurrency limits, and audit log.
- SSRF, payload, egress, and tool-integrity defenses.
- Secret references and initial credential provider.

Exit criterion: identities see and call only permitted tools; sensitive decisions are explainable and
auditable.

### Phase 5 — Control plane

- PostgreSQL storage.
- Administrative API.
- Controllers and versioned configuration snapshots.
- Hot reload and rollback.
- Gateway/control-plane runtime profiles.

Exit criterion: multiple gateways consume the same revision and update without downtime.

### Phase 6 — Operator experience

- Prometheus/OpenTelemetry production exporters.
- Full CLI resource workflow.
- Dashboard and event stream.
- Docker Compose, Kubernetes manifests, and Helm.
- GitOps validation and dry-run.

### Phase 7 — Advanced platform

- Virtual MCP servers and dynamic tool filtering.
- Approval workflows and external secret providers.
- Canary, regional, and adaptive routing.
- Multi-tenancy hardening, mTLS, and multi-cluster operation.
- WASM plugins and federation only after the core platform is stable.

## 16. Dependency rules

Allowed direction:

```text
core + error
     ▲
protocol + config + storage traits
     ▲
registry + router + policy + reliability modules
     ▲
gateway + proxy + control plane + control API
     ▲
runtime + CLI
```

Rules:

1. `core` depends only on minimal serialization/utility libraries.
2. Domain modules do not depend on Axum, SQLx, Redis, NATS, or a concrete telemetry backend.
3. The gateway orchestrates interfaces; it does not own registry, policy, or routing logic.
4. Control-plane storage is never queried on the normal request hot path.
5. Data-plane decisions use immutable snapshots for lock-light reads.
6. Every tenant-scoped operation requires explicit tenant context.
7. External inputs are bounded and validated at their entry boundary.
8. Every administrative mutation is authorized, versioned, and audited.
9. Optional infrastructure has in-memory or standalone alternatives.
10. A crate boundary is justified by ownership and invariants, not merely file count.

## 17. Principal contracts

The main interfaces should converge on contracts similar to these:

```rust
trait Authenticator {
    async fn authenticate(&self, request: &AdmissionRequest) -> Result<Principal>;
}

trait PolicyEngine {
    fn evaluate(&self, context: &RequestContext, action: &Action) -> PolicyDecision;
}

trait CapabilityResolver {
    fn resolve(&self, context: &RequestContext, name: &CapabilityName)
        -> Result<LogicalDestination>;
}

trait Router {
    fn route(&self, context: &RequestContext, destination: &LogicalDestination)
        -> Result<RouteDecision>;
}

trait LoadBalancer {
    fn select(&self, candidates: &[EndpointSnapshot], context: &RequestContext)
        -> Result<EndpointLease>;
}

trait AuditSink {
    async fn record(&self, event: AuditEvent) -> Result<()>;
}
```

Production implementations can be asynchronous where I/O is unavoidable. Pure snapshot lookups and
policy/routing evaluation should remain synchronous and allocation-conscious on the hot path.

## 18. Deployment composition

### Local development

```text
agentmesh single binary
├── embedded gateway
├── embedded control plane
├── SQLite/in-memory state
└── supervised stdio and HTTP MCP servers
```

### Small cloud deployment

```text
load balancer
└── AgentMesh all-in-one replicas
    ├── gateway
    ├── control API
    └── PostgreSQL
```

### Scaled deployment

```text
Control plane replicas ─ PostgreSQL ─ optional Redis/NATS
          │ signed/versioned configuration
          ▼
Gateway fleet by region ─ local MCP services / remote MCP services
```

### Hybrid deployment

A cloud control plane manages gateways inside private networks or developer machines. Gateways make
outbound control connections where possible, while MCP services remain private. Policy determines
which catalogs and metadata may leave each location.

## 19. Module completion criteria

A module is not complete until it has:

- a documented responsibility and explicit non-responsibilities;
- stable domain inputs and outputs;
- unit tests for invariants and failure behavior;
- integration tests at external boundaries;
- bounded resource usage and cancellation behavior;
- tenant-isolation and security review where applicable;
- metrics, traces, safe logs, and auditable actions;
- configuration validation and operational defaults;
- upgrade/compatibility behavior;
- benchmark coverage when it executes on the request hot path.

This catalog is the target platform map. Implementation should follow the delivery phases and avoid
creating empty crates for modules that are not yet scheduled.
