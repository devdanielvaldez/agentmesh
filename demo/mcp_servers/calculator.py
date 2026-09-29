"""MCP 1/5: calculator — arithmetic tools."""
from base import McpHandler, serve


class Handler(McpHandler):
    SERVER_NAME = "calculator"
    TOOLS = [
        {"name": "add", "description": "Add two numbers",
         "inputSchema": {"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}, "required": ["a", "b"]}},
        {"name": "subtract", "description": "Subtract b from a",
         "inputSchema": {"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}, "required": ["a", "b"]}},
        {"name": "multiply", "description": "Multiply two numbers",
         "inputSchema": {"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}, "required": ["a", "b"]}},
        {"name": "divide", "description": "Divide a by b",
         "inputSchema": {"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}, "required": ["a", "b"]}},
    ]
    RESOURCES = [{"uri": "calc://constants/pi", "name": "pi", "description": "Pi constant"}]
    PROMPTS = [{"name": "solve", "description": "Solve an arithmetic word problem"}]

    def handle_tool(self, name, args):
        a, b = float(args["a"]), float(args["b"])
        if name == "add":
            return self.text_result(str(a + b))
        if name == "subtract":
            return self.text_result(str(a - b))
        if name == "multiply":
            return self.text_result(str(a * b))
        if name == "divide":
            if b == 0:
                raise ValueError("division by zero")
            return self.text_result(str(a / b))
        raise ValueError(f"unknown tool: {name}")


if __name__ == "__main__":
    serve(Handler, 3001)
