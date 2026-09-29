# AgentMesh Live Dashboard: GitHub + Filesystem + Memory MCPs behind one gateway

The agent talks to a **single gateway** (`:8080`) configured with **3 native
upstreams**; all traffic goes through AgentMesh, never directly to an MCP server.

| MCP | Sidecar | Tools (subset) |
| --- | --- | --- |
| GitHub (`@modelcontextprotocol/server-github`) | 3001 | search_repositories, file/commits reads… |
| Filesystem (`@modelcontextprotocol/server-filesystem`) | 3002 | list_allowed_directories, list_directory, read_file… |
| Memory (`@modelcontextprotocol/server-memory`) | 3003 | create_entities, read_graph, search_nodes… |

Each official server speaks stdio, so `run.sh` exposes it as Streamable HTTP
via a `supergateway` sidecar (`POST /mcp`), which is exactly what an AgentMesh
static upstream expects. The gateway merges `tools/list` and routes each
`tools/call` to the owning server (see `docs/USER_GUIDE.md` → Multiple
upstreams).

## Requirements

- Rust 1.85+, Python 3.9+ (stdlib only), `curl`, Node 18+ (`npx`).
- A GitHub personal access token (classic or fine-grained, read scopes suffice
  for the tour).
- Optional local Ollama: `ollama serve` + `ollama pull llama3.2:1b`, shown as a
  readiness line.

## Setup

```bash
cp dashboard/.env.example dashboard/.env
# edit dashboard/.env: GITHUB_PERSONAL_ACCESS_TOKEN=...
```

`dashboard/.env` is gitignored and never committed. `GITHUB_TOOLSETS=repos`
keeps the GitHub tour on the read-only repos toolset.

## Run

```bash
./dashboard/run.sh
```

```text
== Starting AgentMesh Gateway ==
✓ GitHub MCP       :3001  ready
✓ Filesystem MCP   :3002  ready
✓ Memory MCP       :3003  ready
✓ AgentMesh        :8080  ready
✓ Ollama           llama3.2:1b

Discovering capabilities...

Tools discovered via AgentMesh: 49

✓ GitHub      search_repositories
✓ Filesystem  list_allowed_directories
✓ Filesystem  list_directory
✓ Filesystem  read_file
✓ Memory      create_entities
✓ Memory      read_graph
✓ Memory      search_nodes

One gateway. Multiple MCP servers.

== Live Dashboard ==
```

The script builds the gateway, starts the 3 sidecars (first start downloads the
NPM packages), waits for TCP and MCP readiness, starts the gateway — whose
startup discovery is fail-closed — runs the agent tour, and finally shows the
live `agentmesh monitor` dashboard for a few seconds. See `docs/USER_GUIDE.md`
(Live monitoring) to keep that dashboard running against your own gateway.

Run without `sudo`: all ports are above 1024, and sudo mixes process
ownership, leaving stale servers behind on reruns. If the script refuses with
`port 300X is already in use`, stop the leftover process (`lsof -i :300X`,
then kill it) and rerun.

## What the agent does (`dashboard/agent.py`)

1. `GET /health/live` on the gateway, then `tools/list` discovery.
2. Silent guards (fail fast and loud): a version-less request must be rejected,
   and retired placeholder servers must not answer on the sidecar ports.
3. A curated, mostly read-only tour: static safe calls plus adaptive chains
   (allowed dirs → list dir → read file; create entity → read graph → search),
   one checkmark line per call.

## Self-test

```bash
python3 dashboard/selftest.py
```

Socket-free consistency checks between `gateway.yaml`, `.env.example`,
`run.sh`, and `agent.py` (no network, no secrets needed).

## AgentMesh capabilities exercised

Native multi-upstream with fail-closed discovery, protocol validation
(`application/json`, version `2026-07-28` in header + `_meta`), credential-safe
proxying, safe error envelope (`CAPABILITY_NOT_FOUND` for unknown tools),
`x-request-id`, live accounting (`GET /metrics`, `agentmesh metrics`,
`agentmesh monitor`), telemetry/tracing.
