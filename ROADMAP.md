# Roadmap

The roadmap is directional and may change as MCP evolves and real deployments provide feedback.

## 0.1 — Functional data plane

- Streamable HTTP proxy and bounded MCP message parsing.
- Static upstream registry and tool discovery.
- Deterministic routing, round-robin balancing, timeouts, and passive health signals. **Complete.**
- Prometheus metrics and OpenTelemetry traces.

## 0.2 — Reliability and policy

- Active health checks, circuit breakers, safe retries, and backpressure. **Complete.**
- Task affinity and independently authorized lifecycle operations. **Complete.**
- API-key authentication, RBAC, local rate limits, and explainable policy decisions. **Complete.**
- Secret brokering, SSRF/integrity defenses, bounded telemetry, cache, and audit outbox. **Complete.**
- External identity providers, distributed backends, and production telemetry/audit exporters.
- Local stdio bridge with supervised process lifecycle.

## 0.3 — Operator experience

- Declarative server and route resources.
- Inspection, validation, and explain commands.
- Dashboard API, Docker Compose stack, Helm chart, and GitOps workflow.

## Beyond

- Virtual MCP servers, tool pinning, approvals, multi-tenancy, regional routing, plugins, federation,
  and a separated high-availability control plane.

The complete design space is documented in [docs/T_ARCHITECTURE.md](docs/T_ARCHITECTURE.md).
