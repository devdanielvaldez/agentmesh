"""AgentMesh live showcase agent: talks to ONE gateway (:8080) fronting 3 MCPs.

Upstreams (via supergateway sidecars): GitHub (:3001), Filesystem (:3002),
Memory (:3003). Flow through the single gateway:
  1. Gateway liveness, then tools/list discovery.
  2. Silent guards: version-less requests must be rejected, and retired
     placeholder servers must not answer on the sidecar ports (both fail
     fast and loud).
  3. A curated, mostly read-only tour: static safe calls plus an adaptive
     chain (allowed dirs -> list dir -> read file; create entity -> read
     graph -> search). Output is checkmark style, one line per call.

Stdlib only.
"""
import json
import os
import urllib.request
import urllib.error

PROTOCOL_VERSION = "2026-07-28"
GATEWAY = os.environ.get("AGENTMESH_URL", "http://127.0.0.1:8080")

# Static safe calls: tool -> arguments. Everything else runs only if the
# adaptive chain discovers it.
STATIC_CALLS = {
    "search_repositories": {"query": "modelcontextprotocol mcp"},
    "create_entities": {"entities": [{
        "name": "AgentMeshLive",
        "entityType": "showcase",
        "observations": ["exercised via the AgentMesh gateway"],
    }]},
    "read_graph": {},
}

# Owning server for every tool the tour can call, for display.
SERVER_OF = {
    "search_repositories": "GitHub",
    "list_allowed_directories": "Filesystem",
    "list_directory": "Filesystem",
    "read_file": "Filesystem",
    "create_entities": "Memory",
    "read_graph": "Memory",
    "search_nodes": "Memory",
}

# Tools from the retired placeholder servers. They must never appear here.
STALE_TOOLS = {
    "add", "subtract", "multiply", "divide",
    "note_create", "note_list", "note_get",
    "weather_get", "cities_list",
    "file_list", "file_read",
    "time_now", "uuid_generate", "echo",
}

_counter = [0]


def mcp_call(method, params=None, include_version=True):
    _counter[0] += 1
    body = {"jsonrpc": "2.0", "id": _counter[0], "method": method,
            "params": params if params is not None else {"_meta": {
                "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
                "io.modelcontextprotocol/clientCapabilities": {}}}}
    if params is not None and "_meta" not in params:
        params["_meta"] = {
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {}}
    req = urllib.request.Request(
        GATEWAY + "/mcp", data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "Accept": "application/json"},
        method="POST")
    if include_version:
        req.add_header("MCP-Protocol-Version", PROTOCOL_VERSION)
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            return ("ok", resp.status, json.loads(resp.read().decode()))
    except urllib.error.HTTPError as exc:
        try:
            detail = json.loads(exc.read().decode())
        except Exception:
            detail = {"raw": "unparseable error body"}
        return ("http-error", exc.code, detail)
    except Exception as exc:
        return ("conn-error", 0, {"error": str(exc)})


def result_text(resp):
    try:
        blocks = resp["result"]["content"]
        return " | ".join(b.get("text", str(b)) for b in blocks)[:300]
    except (KeyError, TypeError):
        return json.dumps(resp)[:300]


def pick_dir(text):
    """Pick a directory from list_allowed_directories output."""
    lines = [ln.strip() for ln in text.strip().splitlines() if ln.strip()]
    paths = [ln for ln in lines if ln.startswith("/")]
    if text.strip().startswith("["):
        return json.loads(text)[0]
    return (paths or lines[-1:])[0]


def pick_text_file(listing):
    """Pick a .txt filename from list_directory output ([FILE] markers ok)."""
    for line in listing.splitlines():
        parts = line.split()
        if parts and parts[-1].endswith(".txt"):
            return parts[-1].strip('"[],')
    return None


def call_tool(tool, arguments):
    """Calls one tool via the gateway; returns (ok, response)."""
    status, _, resp = mcp_call("tools/call", {"name": tool, "arguments": arguments})
    return (status == "ok" and "result" in resp), resp


def show(tool, good, resp):
    """Prints one tour line in checkmark style."""
    server = SERVER_OF.get(tool, "MCP")
    if good:
        print(f"✓ {server:<11} {tool}")
    else:
        print(f"✗ {server:<11} {tool} ({json.dumps(resp)[:120]})")


def main():
    try:
        urllib.request.urlopen(GATEWAY + "/health/live", timeout=5)
    except Exception as exc:
        print(f"✗ gateway not reachable: {exc}")
        return

    print("Discovering capabilities...\n")
    tools = []
    status, _, resp = mcp_call("tools/list")
    if status == "ok":
        try:
            tools = [t["name"] for t in resp["result"]["tools"]]
        except KeyError:
            pass
    if not tools:
        print("✗ no tools discovered through the gateway")
        return

    # Validation guard: a version-less request must be rejected.
    status, _, _ = mcp_call("ping", include_version=False)
    if status == "ok":
        print("✗ gateway accepted a version-less request (validation broken)")
        return

    # Stale-process guard: retired placeholder servers answer on the same ports.
    stale = [t for t in tools if t in STALE_TOOLS]
    if stale:
        print(f"✗ stale placeholder servers hold the sidecar ports (saw: {', '.join(stale)})")
        print("  stop them (lsof -i :3001) and rerun dashboard/run.sh")
        return

    print(f"Tools discovered via AgentMesh: {len(tools)}\n")

    for tool, arguments in STATIC_CALLS.items():
        if tool in tools:
            show(tool, *call_tool(tool, arguments))

    # Adaptive chain 1: filesystem dirs -> list -> read.
    if "list_allowed_directories" in tools:
        good, resp = call_tool("list_allowed_directories", {})
        show("list_allowed_directories", good, resp)
        if good:
            try:
                first_dir = pick_dir(resp["result"]["content"][0]["text"])
                good, resp = call_tool("list_directory", {"path": first_dir})
                show("list_directory", good, resp)
                if good:
                    candidate = pick_text_file(result_text(resp))
                    if candidate:
                        import os as _os
                        good, resp = call_tool(
                            "read_file",
                            {"path": _os.path.join(first_dir, candidate)},
                        )
                        show("read_file", good, resp)
            except (KeyError, IndexError, TypeError):
                pass

    # Adaptive chain 2: memory search over the entity created above.
    if "search_nodes" in tools:
        show("search_nodes", *call_tool("search_nodes", {"query": "AgentMeshLive"}))

    print("\nOne gateway. Multiple MCP servers.")


if __name__ == "__main__":
    main()
