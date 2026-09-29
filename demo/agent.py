"""Demo agent: talks to ONE AgentMesh gateway (:8080) fronting 3 real MCPs.

Upstreams (via supergateway sidecars): GitHub (:3001), Filesystem (:3002),
Memory (:3003). Flow through the single gateway:
  1. GET /health/live (gateway liveness)
  2. POST /mcp ping / tools/list / resources/list / prompts/list
  3. One intentionally invalid request (no MCP-Protocol-Version) to show
     gateway validation rejecting it.
  4. Ask Ollama for a short plan from the discovered tools.
  5. A curated, mostly read-only tour: static safe calls plus an adaptive
     chain (allowed dirs -> list dir -> read file; create entity -> read
     graph -> search). Attempts that an upstream rejects are reported,
     never hidden.

Ollama (OLLAMA_MODEL, default llama3.2:1b) is optional: unreachable means
deterministic fallback mode. Stdlib only.
"""
import json
import os
import urllib.request
import urllib.error

PROTOCOL_VERSION = "2026-07-28"
OLLAMA_URL = os.environ.get("OLLAMA_URL", "http://127.0.0.1:11434")
OLLAMA_MODEL = os.environ.get("OLLAMA_MODEL", "llama3.2:1b")
GATEWAY = os.environ.get("AGENTMESH_URL", "http://127.0.0.1:8080")

# Static safe calls: tool -> arguments. Everything else runs only if the
# adaptive chain discovers it, or is reported as skipped.
STATIC_CALLS = {
    "list_allowed_directories": {},
    "read_graph": {},
    "search_repositories": {"query": "modelcontextprotocol mcp"},
    "create_entities": {"entities": [{
        "name": "AgentMeshDemo",
        "entityType": "demo",
        "observations": ["exercised via the AgentMesh gateway"],
    }]},
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


def ollama_plan(tool_summary):
    prompt = ("You are an agent using MCP tools through AgentMesh. "
              "With these tools:\n" + tool_summary +
              "\nPropose in 5 lines a plan using at least one tool from each server.")
    payload = json.dumps({"model": OLLAMA_MODEL, "prompt": prompt,
                          "stream": False, "options": {"num_predict": 200}}).encode()
    req = urllib.request.Request(OLLAMA_URL + "/api/generate", data=payload,
                                 headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            return json.loads(resp.read().decode()).get("response", "").strip()
    except Exception as exc:
        return f"(Ollama unavailable: {exc}. Deterministic fallback mode.)"


def attempt(label, method, params):
    status, code, resp = mcp_call(method, params)
    good = status == "ok" and "result" in resp
    print(f"[{'OK' if good else 'SKIP'}] {label} -> {result_text(resp) if good else json.dumps(resp)[:200]}")
    return good, resp


def main():
    print("=== AgentMesh demo: 1 gateway + GitHub/Filesystem/Memory MCPs ===\n")
    try:
        with urllib.request.urlopen(GATEWAY + "/health/live", timeout=5) as r:
            print(f"health/live: {r.status}")
    except Exception as exc:
        print(f"health/live FAILED: {exc} (is the gateway up? run demo/run_demo.sh)")
        return

    tools = []
    for method in ("ping", "tools/list", "resources/list", "prompts/list"):
        status, code, resp = mcp_call(method)
        print(f"{method}: {status} http={code} -> {json.dumps(resp)[:160]}")
        if method == "tools/list" and status == "ok":
            try:
                tools = [t["name"] for t in resp["result"]["tools"]]
            except KeyError:
                pass
    status, code, resp = mcp_call("ping", include_version=False)
    print(f"invalid (no version): {status} http={code} -> {json.dumps(resp)[:140]}")
    print(f"\nTools discovered via AgentMesh ({len(tools)}): {', '.join(tools)}\n")

    # Guard against stale processes: the retired toy servers answer on the
    # same sidecar ports. Their tools must never appear here.
    stale = [t for t in tools if t in {
        "add", "subtract", "multiply", "divide",
        "note_create", "note_list", "note_get",
        "weather_get", "cities_list",
        "file_list", "file_read",
        "time_now", "uuid_generate", "echo",
    }]
    if stale:
        print("WARNING: stale toy MCP servers are holding the sidecar ports "
              f"(saw: {', '.join(stale)}).")
        print("Stop them (lsof -i :3001 / ps aux | grep mcp_servers) and rerun "
              "run_demo.sh; results below are NOT from GitHub/Filesystem/Memory.\n")

    print(f"--- Ollama model plan ({OLLAMA_MODEL}) ---")
    print(ollama_plan("\n".join(f"- {t}" for t in tools)) + "\n")

    print("--- Capability tour via gateway ---")
    ok = skipped = 0
    for tool in tools:
        if tool in STATIC_CALLS:
            good, _ = attempt(f"tools/call {tool} {STATIC_CALLS[tool]}",
                              "tools/call", {"name": tool, "arguments": STATIC_CALLS[tool]})
            ok, skipped = ok + good, skipped + (not good)
        else:
            print(f"[SKIP] tools/call {tool} (no safe sample args in this tour)")
            skipped += 1

    # Adaptive chain 1: filesystem dirs -> list -> read.
    if "list_allowed_directories" in tools:
        _, resp = attempt("chain: list_allowed_directories", "tools/call",
                          {"name": "list_allowed_directories", "arguments": {}})
        try:
            text = resp["result"]["content"][0]["text"]
            first_dir = pick_dir(text)
            good, resp = attempt(f"chain: list_directory {first_dir}", "tools/call",
                                 {"name": "list_directory", "arguments": {"path": first_dir}})
            if good:
                listing = result_text(resp)
                candidate = pick_text_file(listing)
                if candidate:
                    import os as _os
                    good, _ = attempt(f"chain: read_file {candidate}", "tools/call",
                                      {"name": "read_file",
                                       "arguments": {"path": _os.path.join(first_dir, candidate)}})
                    ok += good
        except (KeyError, IndexError, TypeError) as exc:
            print(f"[SKIP] filesystem chain aborted: {exc}")

    # Adaptive chain 2: memory search over the entity created above.
    if "search_nodes" in tools:
        good, _ = attempt("chain: search_nodes AgentMeshDemo", "tools/call",
                          {"name": "search_nodes", "arguments": {"query": "AgentMeshDemo"}})
        ok += good

    print(f"\nDone: {ok} successful calls, {skipped} skipped/rejected. See gateway logs for routing.")


if __name__ == "__main__":
    main()
