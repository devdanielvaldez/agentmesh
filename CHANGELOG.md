# Changelog

All notable changes to AgentMesh will be documented here. The project follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
