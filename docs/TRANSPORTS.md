# MCP transports

AgentMesh separates MCP semantics from message delivery. `agentmesh-protocol` owns message meaning;
`agentmesh-transport` owns framing, transport headers, content negotiation, and bounded I/O.

## Streamable HTTP

The HTTP binding currently provides admission rules used by the gateway:

- requires `application/json` request bodies;
- selects JSON or request-scoped SSE responses from `Accept`;
- validates `MCP-Protocol-Version`;
- requires modern protocol versions in both the header and request `_meta`;
- rejects mismatched header and metadata versions;
- supports the legacy `2025-03-26` fallback only when configured by the caller;
- rejects response objects submitted as HTTP requests.

AgentMesh currently configures the gateway for modern `2026-07-28` requests. Legacy transport
fallback is implemented as a reusable binding rule but is not enabled at the public gateway yet.

## Server-Sent Events

SSE messages use `event: message` and one or more `data` fields. Encoding is compact and decoding:

- enforces configured size and protocol limits;
- accepts standard comments, `id`, and `retry` fields;
- joins multiple data lines according to SSE processing rules;
- rejects non-message events and empty events;
- validates the reconstructed JSON-RPC/MCP object.

The proxy streams upstream SSE bodies to callers while enforcing a cumulative response limit.

## stdio

The stdio binding carries one compact JSON-RPC object per line. It operates over generic Tokio
readers and writers, so it can wrap child-process pipes, Unix streams, or in-memory duplex streams.

Safety properties:

- reads at most the configured message bound plus one byte;
- closes the receiving side after a fatal oversized frame;
- rejects empty and malformed frames;
- applies the same depth and collection limits as HTTP;
- flushes every outgoing frame;
- exposes idempotent graceful shutdown;
- retains internal I/O causes without exposing them in client responses.

Child-process spawning, restart policy, environment isolation, and sandboxing belong to the future
process-supervisor module rather than this framing layer.

## Common contract

`McpTransport` is object-safe and supports sending, receiving, identifying the binding, and graceful
close. It returns boxed futures without requiring a procedural-macro dependency. This allows the
proxy and testkit modules to use real stdio transports or future HTTP/in-memory implementations
through the same interface.
