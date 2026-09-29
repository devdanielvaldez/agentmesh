"""MCP 5/5: system — time, uuid, echo utilities."""
import time
import uuid
from base import McpHandler, serve


class Handler(McpHandler):
    SERVER_NAME = "system"
    TOOLS = [
        {"name": "time_now", "description": "Current UTC time",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "uuid_generate", "description": "Generate a random UUID4",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "echo", "description": "Echo a message back",
         "inputSchema": {"type": "object", "properties": {"message": {"type": "string"}}, "required": ["message"]}},
    ]
    RESOURCES = [{"uri": "system://version", "name": "version", "description": "Server version"}]
    PROMPTS = [{"name": "stamp", "description": "Timestamp a message with a UUID"}]

    def handle_tool(self, name, args):
        if name == "time_now":
            return self.text_result(time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()))
        if name == "uuid_generate":
            return self.text_result(str(uuid.uuid4()))
        if name == "echo":
            return self.text_result(f"echo: {args['message']}")
        raise ValueError(f"unknown tool: {name}")


if __name__ == "__main__":
    serve(Handler, 3005)
