# AgentMesh Demo: GitHub + Filesystem + Memory MCPs behind one gateway

The agent talks to a **single gateway** (`:8080`) configured with **3 native
upstreams**; all traffic goes through AgentMesh, never directly to an MCP server.

| MCP | Sidecar | Tools (subset) |
| --- | --- | --- |
| GitHub (`@modelcontextprotocol/server-github`) | 3001 | search_repositories, file/commits reads… |
| Filesystem (`@modelcontextprotocol/server-filesystem`) | 3002 | list_allowed_directories, list_directory, read_file… |
| Memory (`@modelcontextprotocol/server-memory`) | 3003 | create_entities, read_graph, search_nodes… |

Each official server speaks stdio, so `run_demo.sh` exposes it as Streamable
HTTP via a `supergateway` sidecar (`POST /mcp`), which is exactly what an
AgentMesh static upstream expects. The gateway merges `tools/list` and routes
each `tools/call` to the owning server (see `docs/USER_GUIDE.md` → Multiple
upstreams).

## Requirements

- Rust 1.85+, Python 3.9+ (stdlib only), `curl`, Node 18+ (`npx`).
- A GitHub personal access token (classic or fine-grained, read scopes suffice
  for the tour).
- Optional local Ollama: `ollama serve` + `ollama pull llama3.2:1b`. Without
  Ollama the agent falls back to deterministic mode.

## Setup

```bash
cp demo/.env.example demo/.env
# edit demo/.env: GITHUB_PERSONAL_ACCESS_TOKEN=...
```

`demo/.env` is gitignored and never committed. `GITHUB_TOOLSETS=repos` keeps the
GitHub tour on the read-only repos toolset.

## Run

```bash
./demo/run_demo.sh
```

Run without `sudo`: all ports are above 1024, and sudo mixes process
ownership, leaving stale servers behind on reruns. If the script refuses with
`port 300X is already in use`, stop the leftover process
(`lsof -i :300X`, then kill it) and rerun. As a second net, the agent prints a
`WARNING` when it discovers tools from the retired toy servers instead of the
real GitHub/Filesystem/Memory ones.

The script builds the gateway, starts the 3 sidecars (first start downloads the
NPM packages), waits for TCP readiness, starts the gateway — whose startup
discovery is fail-closed — checks Ollama, and runs the agent.

## What the agent does (`demo/agent.py`)

1. `GET /health/live` on the gateway.
2. `ping, tools/list, resources/list, prompts/list` via the gateway (merged responses).
3. One intentionally invalid request (no `MCP-Protocol-Version`) to show gateway rejection.
4. Asks the Ollama model for a plan using the discovered tools.
5. A curated, mostly read-only tour: static safe calls plus adaptive chains
   (allowed dirs → list dir → read file; create entity → read graph → search).
   Rejected attempts are reported, never hidden.

## Self-test

```bash
python3 demo/selftest.py
```

Socket-free consistency checks between `gateway.yaml`, `.env.example`,
`run_demo.sh`, and `agent.py` (no network, no secrets needed).

## AgentMesh capabilities exercised

Native multi-upstream with fail-closed discovery, protocol validation
(`application/json`, version `2026-07-28` in header + `_meta`), credential-safe
proxying, safe error envelope (`CAPABILITY_NOT_FOUND` for unknown tools),
`x-request-id`, telemetry/tracing.
