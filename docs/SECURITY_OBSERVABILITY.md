# Security, credentials, cache, telemetry, and audit

These five modules form the data plane's reusable protection and evidence layer. They contain no
control-plane database queries and work in standalone, cloud, or hybrid compositions.

```text
secret reference ─► credential broker ─┐
                                      │
URL + DNS ─► egress / SSRF guard ─────┼─► protected upstream operation
                                      │                 │
integrity pin ─► trust/quarantine ─────┘                 │
                                                        ├─► revision-aware cache
                                                        ├─► metrics / traces / events
                                                        └─► durable audit outbox
```

## Credential brokering

`agentmesh-credentials` persists only provider, key, and optional version references. Provider
adapters return bounded secret material with an expiry and safe revision. Secret material and leases
redact formatting; owned buffers are overwritten on drop.

Tenant-scoped rules select credentials by logical service and capability using deterministic
priority. The broker caches only until provider expiry, supports explicit revocation and bounded
expiry cleanup, and publishes safe cache/rotation events. Existing leases may finish after cache
revocation, while new requests resolve the rotated revision. The included memory provider supports
local mode and deterministic tests; Vault, Kubernetes, and cloud providers implement the same trait.

## SSRF, egress, payload, and integrity defenses

`agentmesh-security` requires an exact or subdomain host allowlist, allowed ports, approved HTTP(S)
scheme, and a non-empty DNS result. URL credentials, fragments, IP literals, loopback, private,
link-local, carrier-grade NAT, multicast, unspecified, and cloud-metadata ranges fail closed.

A validated destination pins the public DNS set for a request. Re-resolution must remain public and
overlap the pinned set, preventing DNS rebinding. Redirect targets must pass the same validation
before following them.

The module also provides bounded JSON size/depth validation, a strict forwarding-header allowlist,
sensitive metadata redaction, canonical SHA-256 definition fingerprints, constant-time pin
comparison, and explicit `trusted`, `unpinned`, or `quarantine` outcomes.

## MCP-aware cache

`agentmesh-cache` keys every entry by tenant, data family, identity/policy scope, MCP version,
configuration revision, and stable resource reference. Tool calls cannot be represented as a cache
kind. Resource reads require explicit policy opt-in.

TTL, entry bytes, cumulative bytes, and entry count are hard bounded. Expired values are removed on
access, capacity uses least-recently-used eviction, and leases redact payload formatting. Revision
and tenant invalidation prevent stale policy/catalog results after snapshot changes. Metrics expose
hits, misses, evictions, entries, and bytes without request contents.

## Telemetry

`agentmesh-telemetry` parses and emits W3C `traceparent`, rejecting malformed and all-zero IDs.
Metric/event attributes must come from a caller allowlist and pass sensitive-key, length, count, and
control-character checks. Arguments, outputs, tokens, passwords, cookies, and email-like dimensions
are rejected.

The local recorder aggregates count, sum, and maximum for bounded series and buffers safe structured
events. New series beyond capacity fail explicitly; event overflow drops the oldest item and counts
the loss. Prometheus and OpenTelemetry exporters can consume deterministic snapshots without domain
crates depending on a vendor SDK.

## Audit outbox

`agentmesh-audit` records actor, tenant, action, target, outcome, policy/config revision, trace,
timestamp, and a redacted metadata envelope. Every record receives a monotonic sequence and SHA-256
hash linked to its predecessor, allowing corruption/tampering verification before export.

The bounded outbox supports pending batches, stable retry error codes, contiguous acknowledgement,
and two delivery policies. `Required` fails the protected high-risk operation when durability is
unavailable; `BestEffort` continues and increments a dropped counter. PostgreSQL, object storage,
SIEM, webhook, and WORM exporters can preserve this contract in later deployment modules.

## Integrated behavior

The integration test validates an allowed public destination, obtains a redacted upstream
credential, pins a discovered catalog, caches it under tenant/revision scope, records bounded
telemetry, and appends a verified audit record. Secret material is absent from telemetry and audit
representations throughout the flow.
