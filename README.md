<div align="center">

# AgentMesh

### The service mesh for AI tools

**Discover, route, secure, and observe MCP infrastructure at scale.**

[![CI](https://github.com/devdanielvaldez/agentmesh/actions/workflows/ci.yml/badge.svg)](https://github.com/devdanielvaldez/agentmesh/actions/workflows/ci.yml)
[![Security](https://github.com/devdanielvaldez/agentmesh/actions/workflows/security.yml/badge.svg)](https://github.com/devdanielvaldez/agentmesh/actions/workflows/security.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-7c3aed.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.85%2B-f97316?logo=rust)](https://www.rust-lang.org/)
[![Status](https://img.shields.io/badge/status-early%20development-f59e0b)](#project-status)

[Vision](docs/T_ARCHITECTURE.md) · [Quick start](#quick-start) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md)

</div>

---

AgentMesh is an open-source gateway and service mesh for the
[Model Context Protocol](https://modelcontextprotocol.io/). Applications connect to one logical
MCP endpoint while AgentMesh handles the operational complexity behind it.

```text
 AI agents and applications
             │
             ▼
 ┌─────────────────────────┐
 │        AgentMesh        │
 │ Discovery  │  Security  │
 │ Routing    │  Policies  │
 │ Resilience │  Telemetry │
 └────────────┬────────────┘
        ┌─────┼─────┐
        ▼     ▼     ▼
      GitHub Slack Database   MCP servers
```

## Why AgentMesh?

Direct MCP connections work well at small scale. At organizational scale, teams also need server
discovery, consistent authorization, failover, auditability, and a clear view of every tool call.
AgentMesh brings those capabilities to MCP without tying applications to one model provider or MCP
server implementation.

- **One MCP endpoint** for an organization's approved tools.
- **Capability-aware routing** across services, replicas, versions, and regions.
- **Secure by default** with identity, policy enforcement, tool filtering, and credential isolation.
- **Built for failure** with health checks, timeouts, load balancing, and circuit breakers.
- **Observable end to end** through structured logs, metrics, traces, and audit events.
- **Local, cloud, or hybrid** operation through a single binary, containers, and future Kubernetes
  deployments.

## Quick start

AgentMesh is currently an early implementation. The gateway can validate and proxy modern MCP
requests to one statically configured Streamable HTTP upstream.

```bash
git clone https://github.com/devdanielvaldez/agentmesh.git
cd agentmesh
cargo run -p agentmesh -- validate config/agentmesh.yaml
cargo run -p agentmesh -- serve --config config/agentmesh.yaml
```

In another terminal:

```bash
curl --include http://localhost:8080/health/live
curl http://localhost:8080/
```

Or run the containerized development setup:

```bash
docker compose up --build
```

## Architecture

The workspace starts intentionally small and preserves clear boundaries:

```text
crates/
├── agentmesh/          CLI and process lifecycle
├── agentmesh-config/   typed configuration and validation
├── agentmesh-core/     shared domain types
├── agentmesh-discovery/ bounded endpoint and MCP capability discovery
├── agentmesh-error/    stable error taxonomy and safe client responses
├── agentmesh-gateway/  HTTP data plane and middleware
├── agentmesh-protocol/ MCP types, negotiation, and bounded validation
├── agentmesh-proxy/    bounded, credential-safe upstream forwarding
├── agentmesh-registry/ tenant-scoped services, endpoints, and capabilities
└── agentmesh-transport/ bounded HTTP, SSE, and stdio bindings
```

The long-term design separates a configuration-oriented **control plane** from the high-performance
**data plane** that handles MCP traffic. See the complete [technical vision](docs/T_ARCHITECTURE.md)
and the [first architecture decision](docs/adr/0001-workspace-and-boundaries.md).

## Roadmap

- [x] Rust workspace, CLI, configuration, gateway skeleton, health endpoints, and graceful shutdown.
- [x] Streamable HTTP proxy and MCP protocol validation.
- [x] Versioned service registry with in-memory and SQLite persistence.
- [x] Capability discovery and global tool catalog.
- [ ] Routing, load balancing, health checks, and circuit breakers.
- [ ] Authentication, RBAC, policies, and rate limiting.
- [ ] Prometheus metrics, OpenTelemetry traces, and audit events.
- [ ] Virtual MCP servers, dashboard, and Kubernetes deployment.

The detailed sequencing lives in [ROADMAP.md](ROADMAP.md).

## Project status

AgentMesh is **pre-release software under active development**. APIs and configuration may change
before `1.0`. It is not yet suitable for production workloads or sensitive credentials.

## Community

We welcome design feedback, use cases, documentation improvements, and focused pull requests. Read
the [contribution guide](CONTRIBUTING.md), follow the [code of conduct](CODE_OF_CONDUCT.md), and use
GitHub Discussions for open-ended proposals.

## License

AgentMesh is available under the [MIT License](LICENSE).
