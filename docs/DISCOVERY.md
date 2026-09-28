# MCP discovery

`agentmesh-discovery` turns upstream MCP list responses and infrastructure provider output into
bounded, deterministic inputs for the registry. Discovery reports observations; it does not make an
endpoint trusted, healthy, or routable.

## Capability discovery

Each run begins with `server/discover`, validates the advertised protocol revisions, and negotiates
the newest mutually supported modern revision. Only capability families declared by the server are
queried; an incompatible or malformed discovery response fails before catalog reconciliation.

The discovery engine calls the standard paginated methods:

- `tools/list`;
- `resources/list`;
- `resources/templates/list`;
- `prompts/list`.

Opaque cursors are returned to the same method until `nextCursor` is absent. The engine rejects
empty, oversized, or repeated cursors and enforces cumulative page, capability, and per-item limits.
Malformed result shapes and duplicate capability keys are classified as upstream protocol errors.

Tools, resources, templates, and prompts are normalized into the registry's common capability
model. Every upstream item receives a deterministic SHA-256 integrity digest, and the complete
sorted catalog receives a second fingerprint. This makes metadata and schema changes explicit even
when the server returns fields in a different object-key order.

The current MCP `ttlMs` and `cacheScope` hints are preserved per capability family for later refresh
scheduling. Discovery does not treat cache metadata or server self-description as identity.

## Safe reconciliation

`DiscoveryEngine` reads the current service revision, discovers the complete catalog, calculates
added, removed, and changed keys, and asks a `DiscoveryPolicy` for one of three actions:

- accept and store the new catalog;
- store it while quarantining the service;
- reject it without changing registry state.

The default conservative policy accepts a service's first catalog and quarantines subsequent
changes. An explicit accept-all policy is available for controlled development. Registry writes use
the original revision as an optimistic precondition, so a slow discovery run cannot overwrite a
concurrent operator update.

## Endpoint providers

`EndpointProvider` is an object-safe contract that returns tenant-scoped endpoint candidates. The
initial `StaticProvider` supports bounded bootstrap configuration. DNS, container, Kubernetes, and
self-registration providers remain future work. Provider output still requires endpoint validation,
identity verification, policy, and health evaluation before routing.

## HTTP adapter

`ProxyDiscoveryClient` uses the existing credential-safe upstream proxy. Each request carries modern
MCP metadata and protocol headers, uses a unique JSON-RPC identifier, inherits proxy deadlines and
response limits, and accepts only correlated JSON responses. SSE is intentionally unsupported for
finite capability list pages.
