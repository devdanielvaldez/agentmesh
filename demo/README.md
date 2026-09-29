# AgentMesh Demo: 5 MCPs + 1 Multi-Upstream Gateway + Ollama Agent

The agent talks to a **single gateway** (`:8080`) configured with **5 native
upstreams**; all traffic goes through AgentMesh, never directly to an MCP server.

| MCP | Port | Tools |
| --- | --- | --- |
| calculator | 3001 | add, subtract, multiply, divide |
| notes | 3002 | note_create, note_list, note_get |
| weather | 3003 | weather_get, cities_list |
| files | 3004 | file_list, file_read (`demo/data` sandbox) |
| system | 3005 | time_now, uuid_generate, echo |

`demo/gateways/gateway.yaml` lists all 5 under `upstreams`. The gateway merges
`tools/list` and routes each `tools/call` to the owning server (see
`docs/USER_GUIDE.md` → Multiple upstreams).

## Requirements

- Rust 1.85+, Python 3.9+ (stdlib only), `curl`.
- Optional local Ollama: `ollama serve` + `ollama pull llama3.2:1b`
  (or export `OLLAMA_MODEL=qwen2.5-coder:7b`). Without Ollama the agent falls
  back to deterministic mode.

## Run

```bash
./demo/run_demo.sh
```

## What the agent does (`demo/agent.py`)

1. `GET /health/live` on the gateway.
2. `ping, tools/list, resources/list, prompts/list` via the gateway (merged responses).
3. One intentionally invalid request (no `MCP-Protocol-Version`) to show gateway rejection.
4. Asks the Ollama model for a plan using the discovered tools.
5. `tools/call` on **all 14 tools** via the gateway (full capability tour).

## Self-test

```bash
python3 demo/selftest.py
```

Socket-free check of the 5 servers' logic and of the multi-upstream
`gateway.yaml` consistency.

## AgentMesh capabilities exercised

Multi-upstream with fail-closed discovery, protocol validation
(`application/json`, version `2026-07-28` in header + `_meta`), credential-safe
proxying, safe error envelope (`CAPABILITY_NOT_FOUND` for unknown tools),
`x-request-id`, telemetry/tracing. Auth/RBAC/policy/rate-limit/circuit-breaker
exist as modules wired to the runtime via snapshots (see `docs/`).
