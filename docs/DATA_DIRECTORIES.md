# AgentMesh data directories

AgentMesh writes generated and runtime data to the operating system's user data directory, not to
the current project. `AGENTMESH_HOME` remains an explicit override for portable or managed
deployments.

| Platform | Default AgentMesh home |
| --- | --- |
| macOS | `~/Library/Application Support/AgentMesh` |
| Windows | `%LOCALAPPDATA%\AgentMesh` |
| Ubuntu and other Linux distributions | `${XDG_DATA_HOME:-~/.local/share}/agentmesh` |

The layout separates persistent browser sessions from generated MCP packages:

```text
AgentMesh home/
├── sessions/       # persistent browser login profiles
├── mcp/            # generated MCP server packages
├── mcp-state/      # MCP audits, artifacts, repairs, and experience data
├── workflows/      # taught workflow IR
├── runs/           # run metadata and artifacts
├── experiences/
├── repairs/
├── policy-state/
└── tmp/
```

`agentmesh sessions login <app> <url>` stores the browser profile in `sessions/<app>`. A workflow
export without `--out` stores its package in `mcp/<server-name>`:

```bash
agentmesh workflows export linkedin.list_messages --target mcp
agentmesh workflows export --all --target mcp
```

Passing `--out` is an explicit opt-in to another export location. Generated MCP servers use
`mcp-state/<server-name>` for mutable runtime state even when their package was exported elsewhere.
Set `AGENTMESH_MCP_STATE_DIR` to override that runtime location.

## Override

Set one directory to relocate all AgentMesh-owned data:

```bash
export AGENTMESH_HOME=/srv/agentmesh-data
```

On Windows PowerShell:

```powershell
$env:AGENTMESH_HOME = 'D:\AgentMeshData'
```

