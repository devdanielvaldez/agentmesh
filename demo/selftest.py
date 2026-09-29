"""Durable socket-free selftest for the demo (config consistency).

Run:  python3 demo/selftest.py   (stdlib only, exit 1 on failure)
Covers: gateway.yaml lists one upstream per sidecar port, .env.example
documents every variable run_demo.sh requires, sidecar commands match the
official MCP packages, and agent.py covers the advertised tour.
"""
import os
import re
import sys

DEMO = os.path.dirname(os.path.abspath(__file__))
FAILS = []


def check(label, cond):
    print(("OK   " if cond else "FAIL ") + label)
    if not cond:
        FAILS.append(label)


def read(name):
    with open(os.path.join(DEMO, name), encoding="utf-8") as fh:
        return fh.read()


yaml_text = read(os.path.join("gateways", "gateway.yaml"))
ports = re.findall(r"url: http://127\.0\.0\.1:(\d+)/mcp", yaml_text)
check("gateway.yaml has 3 upstreams", len(ports) == 3)
check("gateway.yaml covers sidecar ports 3001-3003", sorted(ports) == ["3001", "3002", "3003"])

env_example = read(".env.example")
run_demo = read("run_demo.sh")
required_vars = ["GITHUB_PERSONAL_ACCESS_TOKEN", "GITHUB_TOOLSETS", "OLLAMA_MODEL"]
for var in required_vars:
    check(f".env.example documents {var}", var in env_example)
    check(f"run_demo.sh consumes {var}", var in run_demo)
check("demo/.env is gitignored", ".env" in read(".gitignore").split())

for package in ("server-github", "server-filesystem", "server-memory"):
    check(f"run_demo.sh bridges {package}", package in run_demo)
check("run_demo.sh uses streamableHttp sidecars", "streamableHttp" in run_demo)
for port in ("3001", "3002", "3003"):
    check(f"run_demo.sh serves sidecar :{port}", port in run_demo)

agent = read("agent.py")
for tool in ("list_allowed_directories", "read_graph", "search_repositories",
             "create_entities", "search_nodes"):
    check(f"agent.py tours {tool}", tool in agent)
check("agent.py goes through the single gateway", 'GATEWAY + "/mcp"' in agent)
check("agent.py picks a path line for the dir chain", 'startswith("/")' in agent)

sys.path.insert(0, DEMO)
import agent as agent_mod
check("pick_dir skips header lines",
      agent_mod.pick_dir("Allowed directories:\n/Users/user/Documents/Projects/AgentMesh/demo/data")
      == "/Users/user/Documents/Projects/AgentMesh/demo/data")
check("pick_dir parses JSON lists", agent_mod.pick_dir('["/a", "/b"]') == "/a")
check("pick_text_file strips markers",
      agent_mod.pick_text_file("[FILE] madrid.txt\n[DIR] sub") == "madrid.txt")
check("pick_text_file empty when no txt", agent_mod.pick_text_file("[DIR] sub") is None)
check("agent.py warns on stale toy tools", "stale toy MCP servers" in agent)
check("run_demo.sh has port preflight", "already in use" in run_demo)
check("run_demo.sh identifies the port holder", "lsof -i" in run_demo)
check("run_demo.sh verifies sidecars stay alive", "kill -0" in run_demo)

print(f"SELFTEST FAILURES: {len(FAILS)}")
sys.exit(1 if FAILS else 0)
