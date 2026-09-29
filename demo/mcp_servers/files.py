"""MCP 4/5: files — sandboxed read/list over demo/data."""
import os
from base import McpHandler, serve

DATA_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "data")


class Handler(McpHandler):
    SERVER_NAME = "files"
    TOOLS = [
        {"name": "file_list", "description": "List files in the demo sandbox",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "file_read", "description": "Read a file from the demo sandbox",
         "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}},
    ]
    RESOURCES = [{"uri": "files://sandbox", "name": "sandbox", "description": "Sandboxed data dir"}]
    PROMPTS = [{"name": "index", "description": "Index the sandbox files"}]

    def handle_tool(self, name, args):
        if name == "file_list":
            return self.text_result("\n".join(sorted(os.listdir(DATA_DIR))))
        if name == "file_read":
            safe = os.path.basename(str(args["name"]))
            path = os.path.join(DATA_DIR, safe)
            if not os.path.isfile(path):
                raise ValueError(f"file not found: {safe}")
            with open(path, encoding="utf-8") as fh:
                return self.text_result(fh.read()[:4000])
        raise ValueError(f"unknown tool: {name}")


if __name__ == "__main__":
    os.makedirs(DATA_DIR, exist_ok=True)
    serve(Handler, 3004)
