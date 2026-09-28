# MCP upstream proxy

`agentmesh-proxy` forwards validated MCP messages to Streamable HTTP servers. The gateway performs
transport admission first, then hands an owned protocol message to a pooled HTTP client.

## Configure a static upstream

The first proxy milestone supports one upstream endpoint. Add it under `gateway`:

```yaml
gateway:
  host: 0.0.0.0
  port: 8080
  upstream:
    url: https://mcp.example.com/mcp
    allow_insecure_http: false
    request_timeout_ms: 30000
```

HTTPS is required by default. Plain HTTP must be explicitly enabled and is intended for local
development. URLs with embedded credentials or fragments are rejected. Registry-based endpoint
selection will replace the static target in a later module.

## Request and response handling

- Requests are decoded and validated before any upstream connection is attempted.
- The shared client pools connections, applies connect and request deadlines, and never follows
  redirects.
- JSON responses are buffered only up to the configured bound and decoded as valid JSON-RPC.
- `text/event-stream` responses are streamed with a cumulative byte limit.
- Empty `202 Accepted` and `204 No Content` responses are preserved.
- Upstream failures map to AgentMesh's stable, disclosure-safe error taxonomy.

## Credential and header boundary

Caller credentials, cookies, host information, forwarding headers, and hop-by-hop headers are not
sent upstream. Only MCP content negotiation, protocol-version, and distributed-tracing headers are
allowed through. Trusted upstream credentials are represented separately and injected after this
filtering step, so caller input cannot replace them.

The credential API is ready for the future secrets module. Static YAML credentials are deliberately
not supported yet, avoiding secrets in configuration files. Debug output redacts credential values
and URL query strings.
