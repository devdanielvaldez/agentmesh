# MCP protocol support

AgentMesh treats MCP protocol semantics separately from HTTP, SSE, WebSocket, and stdio framing. The
`agentmesh-protocol` crate provides the common types and validation used by every transport.

## Supported eras

| Era | Revisions | Lifecycle | Metadata model |
| --- | --- | --- | --- |
| Modern | `2026-07-28` and compatible future revisions | `server/discover` | Version and client capabilities on every request |
| Legacy | Through `2025-11-25` | `initialize` and `notifications/initialized` | Connection-negotiated version and capabilities |

The current preferred revision is `2026-07-28`. AgentMesh models both eras so transport adapters can
implement explicit fallback without mixing connection state into the protocol core.

Primary references:

- [MCP 2026-07-28 specification](https://modelcontextprotocol.io/specification/2026-07-28)
- [Official MCP schema](https://github.com/modelcontextprotocol/modelcontextprotocol/tree/main/schema)

## Implemented foundations

- JSON-RPC 2.0 requests, notifications, success responses, and error responses.
- String and integer request identifiers; `null` identifiers are rejected.
- Known MCP methods with lossless preservation of future or vendor methods.
- Required modern `_meta` keys using their exact reserved wire names.
- Modern `server/discover` and legacy initialization payloads.
- Extensible client/server capability sets and implementation metadata.
- Strict date-based protocol versions and newest-common-version negotiation.
- MCP capability-name validation.
- Stable integration with the AgentMesh error taxonomy.

## Defensive decoding

Message admission is bounded before routing or policy evaluation. Defaults currently enforce:

- maximum message size: 10 MiB;
- maximum JSON nesting depth: 32;
- maximum cumulative object members and array elements: 20,000;
- exact JSON-RPC version `2.0`;
- exactly one valid request, notification, success response, or error response shape.

Transport modules may configure stricter limits. External JSON Schema references will remain disabled
by default when schema validation is added; schema dialect and complexity enforcement belongs in a
later protocol increment.

## Compatibility policy

Unknown capabilities, metadata, lifecycle fields, and vendor methods are preserved where the MCP
specification defines extension points. Invalid top-level JSON-RPC fields are rejected. Supporting a
date-shaped version does not automatically mean AgentMesh implements that revision: negotiation uses
the explicitly configured supported-version set.
