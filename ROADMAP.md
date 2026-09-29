# Roadmap

The roadmap is directional and may change as MCP evolves and real deployments provide feedback.

## 0.1 — Functional data plane

- Streamable HTTP proxy and bounded MCP message parsing.
- Static upstream registry and tool discovery.
- Deterministic routing, round-robin balancing, timeouts, and passive health signals. **Complete.**
- Vendor-neutral bounded metrics, W3C traces, and structured events. **Complete.**

## 0.2 — Reliability and policy

- Active health checks, circuit breakers, safe retries, and backpressure. **Complete.**
- Task affinity and independently authorized lifecycle operations. **Complete.**
- API-key authentication, RBAC, local rate limits, and explainable policy decisions. **Complete.**
- Secret brokering, SSRF/integrity defenses, bounded telemetry, cache, and audit outbox. **Complete.**
- External identity providers, distributed backends, and production telemetry/audit exporters.
- Local stdio bridge with supervised process lifecycle. **Complete.**

## 0.3 — Operator experience

- Declarative desired-state resources, signed snapshots, and rollout tracking. **Complete.**
- Validation, schema, diff, doctor, apply, reconcile, and snapshot CLI commands. **Complete.**
- Authenticated administrative HTTP API and SSE event bootstrap. **Complete.**
- Runtime profiles, ordered lifecycle, storage contracts, and deterministic testkit. **Complete.**
- Dashboard API, Docker Compose stack, Helm chart, and GitOps workflow.

## Beyond

- Virtual MCP servers, production external providers/exporters, dashboard, Kubernetes/Helm,
  federation, and multi-cluster operation. Core plugin and separated control-plane contracts are
  complete; these items are deployment integrations rather than missing workspace modules.

The complete design space is documented in [docs/T_ARCHITECTURE.md](docs/T_ARCHITECTURE.md).
