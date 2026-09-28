# MCP service registry

`agentmesh-registry` owns the authoritative catalog of logical MCP services, their physical
endpoints, and discovered or declared capabilities. It is independent from HTTP, routing, and the
control plane so the same contract works in a local process and in future distributed deployments.

## Data model

Every service belongs to an explicit organization and namespace. A service document contains:

- a stable service identifier and human-readable name;
- administrative lifecycle state (`active`, `draining`, `quarantined`, or `disabled`);
- physical HTTP or stdio endpoints with independent lifecycle, weights, and labels;
- tools, resources, resource templates, prompts, and task handlers;
- optional schemas, discovery integrity digests, descriptions, and operator labels;
- the global registry revision that last changed the document.

Names are case-sensitive and unique within a tenant scope. Stable identifiers remain the source of
identity when a service is renamed.

## Revisions and snapshots

Every successful mutation advances one monotonic global revision. Replacements and removals require
the caller's last observed document revision, preventing lost updates. Create operations reject both
duplicate identifiers and duplicate names.

Routing consumers read an immutable, tenant-scoped `RegistrySnapshot`. Snapshots are returned in
stable name-and-ID order and expose capability lookup without backend access. Consumers can compare
the snapshot revision with their last applied revision and atomically replace local routing state.

## Backends

`InMemoryRegistry` is thread-safe and suitable for tests, ephemeral development, and future
control-plane snapshots. `SqliteRegistry` persists the same complete documents using immediate
write transactions, a versioned schema, uniqueness constraints, and a busy timeout. Both backends
run the same conformance suite.

The SQLite backend is intended for local and single-node operation. A future control-plane storage
implementation will publish versioned snapshots instead of putting database queries on the gateway
request path.

## Resource and security boundaries

- Every operation requires a validated organization and namespace.
- Service, endpoint, capability, label, and document counts or sizes are bounded.
- One service document is stored atomically; partially updated catalogs are never exposed.
- Corrupt persistent documents produce opaque storage errors rather than unchecked routing state.
- Registry records contain credential references only in future versions, never raw secrets.
- Endpoint trust, SSRF policy, active health, and capability discovery belong to later modules.
