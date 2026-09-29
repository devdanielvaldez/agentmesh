"""Shared minimal Streamable-HTTP MCP server base (stdlib only)."""
import json
from http.server import BaseHTTPRequestHandler, HTTPServer


class McpHandler(BaseHTTPRequestHandler):
    server_version = "DemoMCP/0.1"
    # Subclasses override these:
    SERVER_NAME = "demo"
    TOOLS = []          # list of {"name","description","inputSchema"}
    RESOURCES = []      # list of {"uri","name","description"}
    PROMPTS = []        # list of {"name","description"}

    def log_message(self, *args):  # quieter logs
        pass

    def do_GET(self):
        if self.path in ("/", "/health"):
            self._send(200, {"name": self.SERVER_NAME, "status": "ok"})
        else:
            self._send(404, {"error": "not found"})

    def do_POST(self):
        if self.path != "/mcp":
            self._send(404, {"error": "only POST /mcp"})
            return
        try:
            length = int(self.headers.get("Content-Length", 0))
            raw = self.rfile.read(length) if length else b"{}"
            msg = json.loads(raw.decode("utf-8"))
        except Exception as exc:
            self._send_jsonrpc(None, error={"code": -32700, "message": f"parse error: {exc}"})
            return
        req_id = msg.get("id")
        method = msg.get("method", "")
        params = msg.get("params", {}) or {}
        try:
            if method == "ping":
                result = {"ok": True, "server": self.SERVER_NAME}
            elif method == "tools/list":
                result = {"tools": self.TOOLS}
            elif method == "tools/call":
                result = self.handle_tool(params.get("name", ""), params.get("arguments", {}) or {})
            elif method == "resources/list":
                result = {"resources": self.RESOURCES}
            elif method == "prompts/list":
                result = {"prompts": self.PROMPTS}
            elif method == "server/discover":
                result = {"server": self.SERVER_NAME, "tools": self.TOOLS,
                           "resources": self.RESOURCES, "prompts": self.PROMPTS}
            else:
                self._send_jsonrpc(req_id, error={"code": -32601, "message": f"unknown method: {method}"})
                return
            self._send_jsonrpc(req_id, result=result)
        except ValueError as exc:
            self._send_jsonrpc(req_id, error={"code": -32602, "message": str(exc)})
        except Exception as exc:  # never leak internals
            self._send_jsonrpc(req_id, error={"code": -32603, "message": f"internal error: {exc}"})

    # -- helpers ---------------------------------------------------------
    def handle_tool(self, name, args):
        raise NotImplementedError

    def text_result(self, text):
        return {"content": [{"type": "text", "text": text}]}

    def _send(self, status, obj):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_jsonrpc(self, req_id, result=None, error=None):
        obj = {"jsonrpc": "2.0", "id": req_id}
        if error is not None:
            obj["error"] = error
        else:
            obj["result"] = result
        self._send(200, obj)


def serve(handler_cls, port):
    print(f"[{handler_cls.SERVER_NAME}] listening on 127.0.0.1:{port}/mcp", flush=True)
    HTTPServer(("127.0.0.1", port), handler_cls).serve_forever()
