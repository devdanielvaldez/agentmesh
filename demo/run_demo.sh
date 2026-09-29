#!/usr/bin/env bash
# Demo AgentMesh: 5 MCPs (3001-3005) + 1 gateway multi-upstream (8080) + agente.
# Uso: ./demo/run_demo.sh
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEMO="$ROOT/demo"
BIN="$ROOT/target/debug/agentmesh"

echo "== 1. Build agentmesh =="
cargo build --locked -p agentmesh --manifest-path "$ROOT/Cargo.toml" || exit 1

echo "== 2. Start 5 MCP servers =="
PIDS=""
export PYTHONDONTWRITEBYTECODE=1
for s in calculator notes weather files system; do
  (cd "$DEMO/mcp_servers" && python3 -B "$s.py" > "/tmp/agentmesh-demo-$s.log" 2>&1) &
  PIDS="$PIDS $!"
done

echo "== 3. Start 1 AgentMesh gateway =="
"$BIN" serve --config "$DEMO/gateways/gateway.yaml" > /tmp/agentmesh-demo-gateway.log 2>&1 &
PIDS="$PIDS $!"

cleanup() { echo; echo "== stopping demo =="; kill $PIDS 2>/dev/null; }
trap cleanup EXIT INT TERM

echo "== 4. Wait for gateway readiness =="
ready=0
for i in $(seq 1 30); do
  if curl -sf http://127.0.0.1:8080/health/live >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
[ "$ready" = 1 ] && echo "gateway ready" || { echo "gateway NO responde (ver /tmp/agentmesh-demo-gateway.log)"; exit 1; }

echo "== 5. Ollama =="
if curl -sf http://127.0.0.1:11434/api/tags >/dev/null 2>&1; then
  echo "ollama OK (modelo: ${OLLAMA_MODEL:-llama3.2:1b})"
else
  echo "AVISO: ollama no responde en :11434. Inicia 'ollama serve' y"
  echo "  'ollama pull ${OLLAMA_MODEL:-llama3.2:1b}'. El agente seguira en modo fallback."
fi

echo "== 6. Run agent =="
python3 -B "$DEMO/agent.py"
