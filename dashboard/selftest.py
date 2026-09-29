"""Durable socket-free selftest for the live showcase (config consistency).

Run:  python3 dashboard/selftest.py   (stdlib only, exit 1 on failure)
Covers: gateway.yaml lists one upstream per sidecar port, .env.example
documents every variable run.sh requires, sidecar commands match the
official MCP packages, and agent.py covers the advertised tour.
(Naming sweep for the retired folder name is verified by grep in CI/review:
the guard pattern itself would trivially match this file.)
"""
import os
import re
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))
FAILS = []


def check(label, cond):
    print(("OK   " if cond else "FAIL ") + label)
    if not cond:
        FAILS.append(label)


def read(name):
    with open(os.path.join(ROOT, name), encoding="utf-8") as fh:
        return fh.read()


yaml_text = read(os.path.join("gateways", "gateway.yaml"))
ports = re.findall(r"url: http://127\.0\.0\.1:(\d+)/mcp", yaml_text)
check("gateway.yaml has 3 upstreams", len(ports) == 3)
check("gateway.yaml covers sidecar ports 3001-3003", sorted(ports) == ["3001", "3002", "3003"])

env_example = read(".env.example")
run_sh = read("run.sh")
required_vars = ["GITHUB_PERSONAL_ACCESS_TOKEN", "GITHUB_TOOLSETS", "OLLAMA_MODEL"]
for var in required_vars:
    check(f".env.example documents {var}", var in env_example)
    check(f"run.sh consumes {var}", var in run_sh)
check(".env is gitignored", ".env" in read(".gitignore").split())

for package in ("server-github", "server-filesystem", "server-memory"):
    check(f"run.sh bridges {package}", package in run_sh)
check("run.sh uses streamableHttp sidecars", "streamableHttp" in run_sh)
for port in ("3001", "3002", "3003"):
    check(f"run.sh serves sidecar :{port}", port in run_sh)

agent = read("agent.py")
for tool in ("list_allowed_directories", "read_graph", "search_repositories",
             "create_entities", "search_nodes"):
    check(f"agent.py tours {tool}", tool in agent)
check("agent.py goes through the single gateway", 'GATEWAY + "/mcp"' in agent)
check("agent.py prints discovery header", '"Discovering capabilities' in agent)
check("agent.py prints closing tagline", "One gateway. Multiple MCP servers." in agent)

sys.path.insert(0, ROOT)
import agent as agent_mod
check("pick_dir skips header lines",
      agent_mod.pick_dir("Allowed directories:\n/Users/user/Documents/Projects/AgentMesh/dashboard/data")
      == "/Users/user/Documents/Projects/AgentMesh/dashboard/data")
check("pick_dir parses JSON lists", agent_mod.pick_dir('["/a", "/b"]') == "/a")
check("pick_text_file strips markers",
      agent_mod.pick_text_file("[FILE] guide.txt\n[DIR] sub") == "guide.txt")
check("pick_text_file empty when no txt", agent_mod.pick_text_file("[DIR] sub") is None)
check("agent.py guards stale placeholder tools", "stale placeholder servers" in agent)
check("run.sh has port preflight", "already in use" in run_sh)
check("run.sh identifies the port holder", "lsof -i" in run_sh)
check("run.sh verifies sidecars stay alive", "kill -0" in run_sh)
check("run.sh shows the live dashboard", "Live Dashboard" in run_sh)

print(f"SELFTEST FAILURES: {len(FAILS)}")
sys.exit(1 if FAILS else 0)
