"""Durable socket-free selftest for the demo (servers + gateway config).

Run:  python3 demo/selftest.py   (stdlib only, exit 1 on failure)
Covers: every tool handler incl. error edges, and the multi-upstream
gateway.yaml consistency (5 upstreams, one per MCP port).
"""
import os
import re
import sys

DEMO = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(DEMO, "mcp_servers"))
os.makedirs(os.path.join(DEMO, "data"), exist_ok=True)

import calculator
import notes
import weather
import files
import system

FAILS = []


def check(label, cond):
    print(("OK   " if cond else "FAIL ") + label)
    if not cond:
        FAILS.append(label)


def raises_valueerror(fn, *args):
    try:
        fn(*args)
    except ValueError:
        return True
    return False


def inst(cls):
    return cls.__new__(cls)


# --- servers -----------------------------------------------------------
c = inst(calculator.Handler)
check("calc add", "12" in c.handle_tool("add", {"a": 7, "b": 5})["content"][0]["text"])
check("calc subtract", "-3" in c.handle_tool("subtract", {"a": 7, "b": 10})["content"][0]["text"])
check("calc multiply", "35" in c.handle_tool("multiply", {"a": 7, "b": 5})["content"][0]["text"])
check("calc divide", "2" in c.handle_tool("divide", {"a": 10, "b": 5})["content"][0]["text"])
check("calc divide-by-zero", raises_valueerror(c.handle_tool, "divide", {"a": 1, "b": 0}))

n = inst(notes.Handler)
check("notes create", "created note" in
      n.handle_tool("note_create", {"title": "t", "body": "b"})["content"][0]["text"])
check("notes list", "t" in n.handle_tool("note_list", {})["content"][0]["text"])
check("notes get", "b" in n.handle_tool("note_get", {"id": 1})["content"][0]["text"])
check("notes get missing", raises_valueerror(n.handle_tool, "note_get", {"id": 9999}))

w = inst(weather.Handler)
check("weather cities", "madrid" in
      w.handle_tool("cities_list", {})["content"][0]["text"])
check("weather get", "24C" in
      w.handle_tool("weather_get", {"city": "madrid"})["content"][0]["text"])
check("weather unknown city", raises_valueerror(w.handle_tool, "weather_get", {"city": "atlantis"}))

f = inst(files.Handler)
listing = f.handle_tool("file_list", {})["content"][0]["text"]
check("files list", "madrid.txt" in listing and "notas.txt" in listing)
check("files read", "Madrid" in
      f.handle_tool("file_read", {"name": "madrid.txt"})["content"][0]["text"])
check("files traversal contained", raises_valueerror(f.handle_tool, "file_read", {"name": "../Cargo.toml"}))
check("files missing", raises_valueerror(f.handle_tool, "file_read", {"name": "nope.txt"}))

s = inst(system.Handler)
check("system time", "T" in s.handle_tool("time_now", {})["content"][0]["text"])
check("system uuid", len(s.handle_tool("uuid_generate", {})["content"][0]["text"]) == 36)
check("system echo", "hola" in s.handle_tool("echo", {"message": "hola"})["content"][0]["text"])

# --- gateway config ----------------------------------------------------
with open(os.path.join(DEMO, "gateways", "gateway.yaml"), encoding="utf-8") as fh:
    yaml_text = fh.read()
ports = re.findall(r"url: http://127\.0\.0\.1:(\d+)/mcp", yaml_text)
check("gateway.yaml has 5 upstreams", len(ports) == 5)
check("gateway.yaml covers MCP ports 3001-3005", sorted(ports) == ["3001", "3002", "3003", "3004", "3005"])
check("gateway.yaml uses upstreams list (no router hop)",
      "upstreams:" in yaml_text and "3099" not in yaml_text)

print(f"SELFTEST FAILURES: {len(FAILS)}")
sys.exit(1 if FAILS else 0)
