#!/usr/bin/env bash
# AgentMesh live showcase: GitHub + Filesystem + Memory MCPs behind 1 gateway.
# Each official MCP server (stdio) is exposed as Streamable HTTP via a
# supergateway sidecar (:3001-:3003); the AgentMesh gateway (:8080) fans out.
# Usage: ./dashboard/run.sh
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHOWCASE="$ROOT/dashboard"
BIN="$ROOT/target/debug/agentmesh"

if [ -f "$SHOWCASE/.env" ]; then
  # shellcheck disable=SC1091
  . "$SHOWCASE/.env"
fi
: "${GITHUB_PERSONAL_ACCESS_TOKEN:?Set GITHUB_PERSONAL_ACCESS_TOKEN in dashboard/.env (see dashboard/.env.example)}"
: "${GITHUB_TOOLSETS:=repos}"
: "${OLLAMA_MODEL:=llama3.2:1b}"
export GITHUB_PERSONAL_ACCESS_TOKEN GITHUB_TOOLSETS OLLAMA_MODEL

command -v node >/dev/null 2>&1 || { echo "node >= 18 required (https://nodejs.org/)"; exit 1; }
command -v npx >/dev/null 2>&1 || { echo "npx required (ships with node)"; exit 1; }

cargo build --locked -p agentmesh --manifest-path "$ROOT/Cargo.toml" || exit 1

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

echo "== Starting AgentMesh Gateway =="
PIDS=""
npx -y supergateway --stdio "npx -y @modelcontextprotocol/server-github" \
  --outputTransport streamableHttp --port 3001 > /tmp/agentmesh-github.log 2>&1 &
PIDS="$PIDS $!"
npx -y supergateway --stdio "npx -y @modelcontextprotocol/server-filesystem $SHOWCASE/data" \
  --outputTransport streamableHttp --port 3002 > /tmp/agentmesh-filesystem.log 2>&1 &
PIDS="$PIDS $!"
npx -y supergateway --stdio "npx -y @modelcontextprotocol/server-memory" \
  --outputTransport streamableHttp --port 3003 > /tmp/agentmesh-memory.log 2>&1 &
PIDS="$PIDS $!"

cleanup() { kill $PIDS 2>/dev/null; }
trap cleanup EXIT INT TERM

mcp_ready() {
  curl -sf "http://127.0.0.1:$1/mcp" \
    -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' \
    -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' \
    2>/dev/null | grep -q '"result"'
}

names="GitHub Filesystem Memory"
i=1
for p in 3001 3002 3003; do
  name=$(echo $names | cut -d' ' -f$i)
  for _ in $(seq 1 150); do
    (echo > "/dev/tcp/127.0.0.1/$p") >/dev/null 2>&1 && mcp_ready "$p" && break
    sleep 2
  done
  mcp_ready "$p" \
    || { echo "✗ $name MCP :$p never answered (see /tmp/agentmesh-*.log)"; exit 1; }
  printf '✓ %-16s :%-5s ready\n' "$name MCP" "$p"
  i=$((i + 1))
done
for pid in $PIDS; do
  kill -0 "$pid" 2>/dev/null \
    || { echo "✗ a sidecar died on startup (see /tmp/agentmesh-*.log)"; exit 1; }
done

"$BIN" serve --config "$SHOWCASE/gateways/gateway.yaml" > /tmp/agentmesh-gateway.log 2>&1 &
PIDS="$PIDS $!"
for _ in $(seq 1 30); do
  curl -sf http://127.0.0.1:8080/health/live >/dev/null 2>&1 && break
  sleep 1
done
curl -sf http://127.0.0.1:8080/health/live >/dev/null 2>&1 \
  || { echo "✗ AgentMesh :8080 not ready (see /tmp/agentmesh-gateway.log)"; exit 1; }
printf '✓ %-16s :%-5s ready\n' "AgentMesh" "8080"

export OLLAMA_MODEL
if curl -sf http://127.0.0.1:11434/api/tags >/dev/null 2>&1; then
  printf '✓ %-16s %s\n' "Ollama" "$OLLAMA_MODEL"
else
  printf '○ %-16s not detected (continuing without it)\n' "Ollama"
fi
echo

python3 -B "$SHOWCASE/agent.py"

echo
echo "== Live Dashboard =="
"$BIN" monitor --gateway http://127.0.0.1:8080 --interval-ms 1000 &
MON=$!
PIDS="$PIDS $MON"
sleep 4
kill $MON 2>/dev/null
wait $MON 2>/dev/null || true
echo
echo "See docs/USER_GUIDE.md (Live monitoring) to keep this dashboard running."
