# Changelog

All notable changes to AgentMesh will be documented here. The project follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
