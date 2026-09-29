#!/usr/bin/env bash
# Demo AgentMesh: GitHub + Filesystem + Memory MCPs behind 1 multi-upstream gateway.
# Each official MCP server (stdio) is exposed as Streamable HTTP via a
# supergateway sidecar (:3001-:3003); the AgentMesh gateway (:8080) fans out.
# Usage: ./demo/run_demo.sh
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEMO="$ROOT/demo"
BIN="$ROOT/target/debug/agentmesh"

if [ -f "$DEMO/.env" ]; then
  # shellcheck disable=SC1091
  . "$DEMO/.env"
fi
: "${GITHUB_PERSONAL_ACCESS_TOKEN:?Set GITHUB_PERSONAL_ACCESS_TOKEN in demo/.env (see demo/.env.example)}"
: "${GITHUB_TOOLSETS:=repos}"
: "${OLLAMA_MODEL:=llama3.2:1b}"
export GITHUB_PERSONAL_ACCESS_TOKEN GITHUB_TOOLSETS OLLAMA_MODEL

command -v node >/dev/null 2>&1 || { echo "node >= 18 required (https://nodejs.org/)"; exit 1; }
command -v npx >/dev/null 2>&1 || { echo "npx required (ships with node)"; exit 1; }

echo "== 1. Build agentmesh =="
cargo build --locked -p agentmesh --manifest-path "$ROOT/Cargo.toml" || exit 1

echo "== 2. Preflight: sidecar/gateway ports must be free =="
for p in 3001 3002 3003 8080; do
  if (echo > "/dev/tcp/127.0.0.1/$p") >/dev/null 2>&1; then
    echo "port $p is already in use. Holder:"
    if command -v lsof >/dev/null 2>&1; then
      lsof -i :"$p" -sTCP:LISTEN 2>/dev/null || lsof -i :"$p" 2>/dev/null || true
    elif command -v ss >/dev/null 2>&1; then
      ss -ltnp 2>/dev/null | grep ":$p " || true
    else
      echo "(install lsof to identify the holder automatically)"
    fi
    echo "Stop that process and rerun. Refusing to start on occupied ports."
    exit 1
  fi
done
echo "ports 3001-3003, 8080 free"

echo "== 3. Start MCP sidecars (stdio -> Streamable HTTP) =="
PIDS=""
npx -y supergateway --stdio "npx -y @modelcontextprotocol/server-github" \
  --outputTransport streamableHttp --port 3001 > /tmp/agentmesh-demo-github.log 2>&1 &
PIDS="$PIDS $!"
npx -y supergateway --stdio "npx -y @modelcontextprotocol/server-filesystem $DEMO/data" \
  --outputTransport streamableHttp --port 3002 > /tmp/agentmesh-demo-filesystem.log 2>&1 &
PIDS="$PIDS $!"
npx -y supergateway --stdio "npx -y @modelcontextprotocol/server-memory" \
  --outputTransport streamableHttp --port 3003 > /tmp/agentmesh-demo-memory.log 2>&1 &
PIDS="$PIDS $!"

cleanup() { echo; echo "== stopping demo =="; kill $PIDS 2>/dev/null; }
trap cleanup EXIT INT TERM

echo "== 4. Wait for sidecars (TCP) =="
for p in 3001 3002 3003; do
  for _ in $(seq 1 90); do
    (echo > "/dev/tcp/127.0.0.1/$p") >/dev/null 2>&1 && break
    sleep 2
  done
  (echo > "/dev/tcp/127.0.0.1/$p") >/dev/null 2>&1 \
    || { echo "sidecar :$p not listening (see /tmp/agentmesh-demo-*.log)"; exit 1; }
  echo "sidecar :$p up"
done
for pid in $PIDS; do
  kill -0 "$pid" 2>/dev/null \
    || { echo "a sidecar died on startup (see /tmp/agentmesh-demo-*.log)"; exit 1; }
done

echo "== 4b. Wait for sidecar MCP readiness (covers npx cold start) =="
for p in 3001 3002 3003; do
  ready=0
  for _ in $(seq 1 60); do
    if curl -sf "http://127.0.0.1:$p/mcp" \
      -H 'Content-Type: application/json' \
      -H 'Accept: application/json, text/event-stream' \
      -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' \
      2>/dev/null | grep -q '"result"'; then
      ready=1
      break
    fi
    sleep 3
  done
  [ "$ready" = 1 ] \
    || { echo "sidecar :$p never answered tools/list (see /tmp/agentmesh-demo-*.log)"; exit 1; }
  echo "sidecar :$p speaks MCP"
done

echo "== 5. Start AgentMesh gateway (discovery is fail-closed) =="
"$BIN" serve --config "$DEMO/gateways/gateway.yaml" > /tmp/agentmesh-demo-gateway.log 2>&1 &
PIDS="$PIDS $!"
for _ in $(seq 1 30); do
  curl -sf http://127.0.0.1:8080/health/live >/dev/null 2>&1 && break
  sleep 1
done
curl -sf http://127.0.0.1:8080/health/live >/dev/null 2>&1 \
  || { echo "gateway not ready (see /tmp/agentmesh-demo-gateway.log)"; exit 1; }
echo "gateway ready"

echo "== 6. Ollama =="
export OLLAMA_MODEL
if curl -sf http://127.0.0.1:11434/api/tags >/dev/null 2>&1; then
  echo "ollama OK (model: $OLLAMA_MODEL)"
else
  echo "NOTE: ollama not responding on :11434. Start 'ollama serve' and"
  echo "  'ollama pull $OLLAMA_MODEL'. The agent continues in fallback mode."
fi

echo "== 7. Run agent =="
python3 -B "$DEMO/agent.py"
