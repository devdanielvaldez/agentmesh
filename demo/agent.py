"""Demo agent: talks to ONE AgentMesh gateway (:8080) fronting 5 MCPs.

Flow through the single gateway:
  1. GET /health/live (gateway liveness)
  2. POST /mcp ping / tools/list / resources/list / prompts/list
  3. One intentionally invalid request (no MCP-Protocol-Version) to show
     gateway validation rejecting it.
  4. Ask Ollama for a short plan from the discovered tools.
  5. POST /mcp tools/call for EVERY listed tool (full capability tour).

Ollama (default model llama3.2:1b, override with OLLAMA_MODEL) is optional:
if unreachable, the agent continues in deterministic fallback mode.
Stdlib only.
"""
import json
import os
import urllib.request
import urllib.error

PROTOCOL_VERSION = "2026-07-28"
OLLAMA_URL = os.environ.get("OLLAMA_URL", "http://127.0.0.1:11434")
OLLAMA_MODEL = os.environ.get("OLLAMA_MODEL", "llama3.2:1b")
GATEWAY = os.environ.get("AGENTMESH_URL", "http://127.0.0.1:8080")

# Server owning each tool (router.py resolves the same map live).
TOOL_OWNER = {
    "add": "calculator", "subtract": "calculator",
    "multiply": "calculator", "divide": "calculator",
    "note_create": "notes", "note_list": "notes", "note_get": "notes",
    "weather_get": "weather", "cities_list": "weather",
    "file_list": "files", "file_read": "files",
    "time_now": "system", "uuid_generate": "system", "echo": "system",
}

# Sample args so EVERY tool gets called at least once.
SAMPLE_ARGS = {
    "add": {"a": 7, "b": 5},
    "subtract": {"a": 7, "b": 5},
    "multiply": {"a": 7, "b": 5},
    "divide": {"a": 7, "b": 5},
    "note_create": {"title": "demo", "body": "nota creada por el agente via AgentMesh"},
    "note_list": {},
    "note_get": {"id": 1},
    "weather_get": {"city": "madrid"},
    "cities_list": {},
    "file_list": {},
    "file_read": {"name": "madrid.txt"},
    "time_now": {},
    "uuid_generate": {},
    "echo": {"message": "hola AgentMesh"},
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
        with urllib.request.urlopen(req, timeout=30) as resp:
            return ("ok", resp.status, json.loads(resp.read().decode()))
    except urllib.error.HTTPError as exc:
        try:
            detail = json.loads(exc.read().decode())
        except Exception:
            detail = {"raw": "unparseable error body"}
        return ("http-error", exc.code, detail)
    except Exception as exc:
        return ("conn-error", 0, {"error": str(exc)})


def ollama_plan(tool_summary):
    prompt = ("Eres un agente que usa herramientas MCP via AgentMesh. "
              "Con estas herramientas:\n" + tool_summary +
              "\nPropon en 5 lineas un plan que use al menos una herramienta de cada servidor.")
    payload = json.dumps({"model": OLLAMA_MODEL, "prompt": prompt,
                          "stream": False, "options": {"num_predict": 200}}).encode()
    req = urllib.request.Request(OLLAMA_URL + "/api/generate", data=payload,
                                 headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            return json.loads(resp.read().decode()).get("response", "").strip()
    except Exception as exc:
        return f"(Ollama no disponible: {exc}. Modo fallback determinista.)"


def main():
    print("=== AgentMesh demo: 1 gateway + 5 MCPs + agente Ollama ===\n")
    print(f"--- gateway {GATEWAY} ---")
    try:
        with urllib.request.urlopen(GATEWAY + "/health/live", timeout=5) as r:
            print(f"health/live: {r.status}")
    except Exception as exc:
        print(f"health/live FALLO: {exc} (¿gateway caido? ejecuta demo/run_demo.sh)")
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
    print(f"invalid (sin version): {status} http={code} -> {json.dumps(resp)[:140]}")
    print()

    summary = "\n".join(f"- {TOOL_OWNER.get(t, '?')}/{t}" for t in tools) or "(sin herramientas)"
    print(f"Herramientas descubiertas via AgentMesh ({len(tools)}):\n{summary}\n")
    print(f"--- Plan del modelo Ollama ({OLLAMA_MODEL}) ---")
    print(ollama_plan(summary) + "\n")

    print("--- Tour completo: tools/call de TODAS las herramientas via gateway ---")
    ok, fail = 0, 0
    for tool in tools:
        if tool == "note_get":  # needs at least one note on a fresh server
            mcp_call("tools/call", {"name": "note_create",
                                    "arguments": SAMPLE_ARGS["note_create"]})
        status, code, resp = mcp_call("tools/call", {"name": tool,
                                                     "arguments": SAMPLE_ARGS.get(tool, {})})
        good = status == "ok" and "result" in resp
        ok, fail = ok + good, fail + (not good)
        srv = TOOL_OWNER.get(tool, "?")
        print(f"[{'OK' if good else 'FALLO'}] {srv}/{tool} "
              f"{SAMPLE_ARGS.get(tool, {})} -> {json.dumps(resp)[:220]}")
    print(f"\nResultado: {ok} OK, {fail} fallos de {len(tools)} llamadas. Demo terminada.")


if __name__ == "__main__":
    main()
