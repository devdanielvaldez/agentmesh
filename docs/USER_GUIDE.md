# AgentMesh user guide

This guide walks through the two executable surfaces available today:

1. the MCP gateway, which validates and proxies traffic to an upstream server;
2. the local control plane, which stores desired resources and compiles immutable snapshots.

## Install

### Build from source

```bash
git clone https://github.com/devdanielvaldez/agentmesh.git
cd agentmesh
cargo build --release --locked -p agentmesh
./target/release/agentmesh --help
export PATH="$PWD/target/release:$PATH"
```

The minimum supported Rust version is declared in `rust-toolchain.toml` and enforced in CI.

### Use the container image locally

```bash
docker build --tag agentmesh:local .
docker run --rm --read-only \
  --publish 8080:8080 \
  --volume "$PWD/config/agentmesh.yaml:/etc/agentmesh/agentmesh.yaml:ro" \
  agentmesh:local
```

## Gateway workflow

### Configure the listener and upstream

```yaml
gateway:
  host: 127.0.0.1
  port: 8080
  upstream:
    url: http://127.0.0.1:3001/mcp
    allow_insecure_http: true
    request_timeout_ms: 30000

telemetry:
  json: false
  filter: agentmesh=debug,tower_http=info
```

`allow_insecure_http` defaults to `false`. Keep that default for remote upstreams. Static upstream
URLs are validated before the listener starts.

### Validate and inspect

```bash
agentmesh validate config/agentmesh.local.yaml
agentmesh doctor --config config/agentmesh.local.yaml
agentmesh schema > agentmesh.schema.json
agentmesh diff config/current.yaml config/candidate.yaml
```

The commands have these failure semantics:

- `validate` fails on unreadable YAML, unknown keys, invalid addresses, port zero, empty upstreams,
  and zero request timeouts;
- `doctor` prints resolved listener and upstream presence after validation;
- `schema` writes JSON Schema to standard output;
- `diff` validates both inputs before reporting a normalized YAML difference.

### Start and stop

```bash
agentmesh serve --config config/agentmesh.local.yaml
```

AgentMesh listens for `SIGINT` and `SIGTERM`, stops accepting new work, and lets Axum perform a
graceful server shutdown.

### Send an MCP request

Modern MCP requests carry the protocol version in both the HTTP header and `_meta` object. Replace
the example version with one listed in [Protocol Support](PROTOCOL_SUPPORT.md) if it changes.

```bash
curl --fail-with-body http://127.0.0.1:8080/mcp \
  --header 'Content-Type: application/json' \
  --header 'Accept: application/json' \
  --header 'MCP-Protocol-Version: 2026-07-28' \
  --data '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/list",
    "params": {
      "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
      }
    }
  }'
```

The gateway returns a disclosure-safe error envelope if no upstream is configured, the transport
binding is invalid, or the upstream fails.

## Control-plane workflow

### Start the service

```bash
export AGENTMESH_ADMIN_TOKEN='replace-with-a-long-random-token'
agentmesh control-plane \
  --host 127.0.0.1 \
  --port 8081 \
  --database agentmesh-control.db
```

The SQLite schema is created idempotently. The service refuses port zero and tokens shorter than 16
bytes. Keep the database on durable storage and protect file permissions.

### Authenticate

Every current `/v1` route requires a bearer token, including status and event endpoints:

```bash
curl --fail \
  --header "Authorization: Bearer ${AGENTMESH_ADMIN_TOKEN}" \
  http://127.0.0.1:8081/v1/status
```

### Apply a resource

Use the included example:

```bash
export AGENTMESH_CONTROL_URL='http://127.0.0.1:8081'

agentmesh apply \
  --tenant acme \
  --namespace development \
  examples/control-plane/route.json
```

`--control-url` and `--token` can be omitted when `AGENTMESH_CONTROL_URL` and
`AGENTMESH_ADMIN_TOKEN` are set. The first write uses create semantics.

For an update, include the revision returned by the previous write:

```bash
agentmesh apply \
  --tenant acme \
  --namespace development \
  --revision 1 \
  examples/control-plane/route.json
```

If another writer has already changed the resource, the API returns `409 Conflict`. Read the latest
revision before retrying; do not blindly overwrite concurrent changes.

### Reconcile and read a snapshot

```bash
agentmesh reconcile --tenant acme --namespace development
agentmesh snapshot --tenant acme --namespace development
```

Reconciliation loads bounded desired state, validates every resource, sorts it canonically, removes
disabled entries, calculates a SHA-256 digest, and atomically activates the new revision.

```mermaid
sequenceDiagram
    actor Operator
    participant CLI
    participant API as Control API
    participant DB as SQLite store
    participant CP as Reconciler

    Operator->>CLI: apply route.json
    CLI->>API: POST resource + bearer token
    API->>DB: conditional write
    DB-->>API: revision
    API-->>CLI: revision response
    Operator->>CLI: reconcile
    CLI->>API: POST reconcile
    API->>CP: compile tenant scope
    CP->>DB: list desired resources
    CP-->>API: immutable snapshot + digest
    API-->>CLI: active revision
```

### Use the API directly

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/v1/status` | Service status and API version. |
| `POST` | `/v1/scopes/{tenant}/{namespace}/resources` | Create or update desired state. |
| `POST` | `/v1/scopes/{tenant}/{namespace}/reconcile` | Compile and activate a snapshot. |
| `GET` | `/v1/scopes/{tenant}/{namespace}/snapshot` | Read the active snapshot. |
| `GET` | `/v1/events` | Open the SSE event bootstrap. |

The maximum request body is 1 MiB. Tenant, namespace, kind, name, document, list, and snapshot sizes
are bounded independently.

## Logging

Use human-readable logs locally:

```yaml
telemetry:
  json: false
  filter: agentmesh=debug,tower_http=info
```

Use structured logs in a container or log pipeline:

```yaml
telemetry:
  json: true
  filter: agentmesh=info,tower_http=warn
```

`RUST_LOG` overrides the configured filter at process startup. Never enable verbose dependency logs
without checking whether the surrounding infrastructure might emit request data.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| `MCP_PROXY_NOT_CONFIGURED` | Add `gateway.upstream` or use the gateway only for health checks. |
| `INVALID_REQUEST` | Verify content type, `Accept`, protocol header, JSON-RPC shape, and `_meta`. |
| Upstream rejected at startup | Use HTTPS, or explicitly allow HTTP for a loopback development server. |
| `401 Unauthorized` from control API | Check the bearer prefix and exact `AGENTMESH_ADMIN_TOKEN` value. |
| `409 Conflict` on apply | Pass the current resource revision with `--revision`. |
| Snapshot returns `404` | Apply at least one resource, then run `reconcile` for the same scope. |
| Address already in use | Change `gateway.port` or the control-plane `--port`. |
| No logs | Set `RUST_LOG=agentmesh=debug,tower_http=info`. |

For operational checks and deployment guidance, continue with [Deployment and Operations](OPERATIONS.md).
