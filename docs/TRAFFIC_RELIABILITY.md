# Traffic routing and reliability

AgentMesh separates five decisions that are often coupled in a proxy: logical routing, physical
endpoint selection, operational health, circuit admission, and request-level resilience. Keeping
these modules independent makes snapshots deterministic and lets local, cloud, and hybrid runtimes
compose the same hot-path contracts without querying control-plane storage.

```text
registry snapshot + request context
              │
              ▼
      deterministic router
              │ candidate endpoints
              ▼
       health eligibility
              │ routable endpoints
              ▼
        load balancer ── endpoint lease
              │
              ▼
    circuit + bulkhead + deadline
              │
              ▼
             proxy
              │ observations
              └────► health / EWMA / circuit
```

## Deterministic routing

`agentmesh-router` compiles a tenant-scoped route table against a registry revision. Higher priority
wins, followed by matcher specificity and stable rule ID ordering. Equal-precedence rules with the
same match and different destinations are rejected at compile time. A decision verifies the tenant,
snapshot revision, active service lifecycle, exact capability advertisement, and endpoint labels.

The router returns an explanation, registry revision, traffic policy, and candidate pool. It does
not inspect live health, pick a replica, authenticate a caller, or perform network I/O. Tables are
bounded to 10,000 rules and external strings are length- and control-character validated.

## Endpoint selection

`agentmesh-load-balancer` supports round robin, deterministic pseudorandom selection, least active
requests, endpoint weights, consistent-hash affinity, EWMA least latency, and adaptive weighted
scoring. Pools accept only unique, active, positive-weight endpoints and are bounded to 10,000
members.

Selection returns an RAII lease. Active-request accounting is released on success, error,
cancellation, or panic. Optional completion observations update bounded request, failure, and EWMA
metrics without storing arguments, results, identities, or other high-cardinality request data.

## Health management

`agentmesh-health` combines scheduled probes and passive request results in a bounded rolling
window. Consecutive failures eject an endpoint; successful trials pass through recovery warm-up;
failure ratio and EWMA latency can degrade it. Old observations become `unknown` so stale success
cannot keep an endpoint routable indefinitely.

Administrative `draining`, `disabled`, and `quarantined` states cannot be overwritten by runtime
observations. Security quarantine remains distinct from operational unhealthiness. Probe times use
stable per-endpoint jitter, and state changes are exposed through a bounded event queue for future
telemetry and audit exporters.

## Circuit breakers

`agentmesh-circuit-breaker` owns one independent closed/open/half-open state machine per endpoint
and, optionally, capability. A bounded rolling failure ratio opens the circuit after minimum
throughput. Monotonic cooldown expiry admits only a configured number of half-open trials; success
closes the circuit after a threshold and any failed trial reopens it.

Permits release half-open capacity when abandoned. Rejections use the stable `CircuitOpen` error,
and snapshots expose only low-cardinality counters and state. Distributed runtimes may tolerate
bounded local divergence or replicate these inputs later; the module itself performs no storage or
network I/O.

## Request resilience

`agentmesh-resilience` provides:

- total deadline budgets that cap every nested operation;
- retry classification gated by safe or verified idempotency;
- capped exponential backoff with deterministic jitter;
- local token buckets that limit retry amplification;
- bounded FIFO queues with high, normal, and background priority;
- concurrency bulkheads with RAII release;
- fail-fast draining and explicit resume.

Unsafe mutations never retry automatically. Full queues and bulkheads reject work instead of growing
memory without bound. Cluster-wide quotas and coordination belong to the future distributed limit
backend, not these local traffic primitives.

## Composition and operations

The crates expose validated defaults but do not create background threads, clocks, or network
clients. Callers supply monotonic timestamps, run probes, publish events, and decide how settings
arrive from configuration snapshots. This keeps unit tests deterministic and allows a single binary
or a fleet of gateways to use identical semantics.

The integration suite exercises the complete path: a route resolves a capability, an unhealthy
replica is removed, the balancer leases the remaining endpoint, circuit and bulkhead permits protect
the attempt, and completion updates all accounting without leaks.
