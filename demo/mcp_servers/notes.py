"""MCP 2/5: notes — tiny in-memory note store."""
from base import McpHandler, serve


class Handler(McpHandler):
    SERVER_NAME = "notes"
    NOTES = {}
    NEXT_ID = 1
    TOOLS = [
        {"name": "note_create", "description": "Create a note with title and body",
         "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}, "body": {"type": "string"}}, "required": ["title", "body"]}},
        {"name": "note_list", "description": "List all note titles",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "note_get", "description": "Get a note by id",
         "inputSchema": {"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}},
    ]
    RESOURCES = [{"uri": "notes://count", "name": "count", "description": "Number of notes"}]
    PROMPTS = [{"name": "summarize", "description": "Summarize all notes"}]

    def handle_tool(self, name, args):
        if name == "note_create":
            nid = Handler.NEXT_ID
            Handler.NEXT_ID += 1
            Handler.NOTES[nid] = {"id": nid, "title": args["title"], "body": args["body"]}
            return self.text_result(f"created note {nid}: {args['title']}")
        if name == "note_list":
            titles = [f"{n['id']}: {n['title']}" for n in Handler.NOTES.values()] or ["(empty)"]
            return self.text_result("\n".join(titles))
        if name == "note_get":
            note = Handler.NOTES.get(int(args["id"]))
            if note is None:
                raise ValueError(f"note {args['id']} not found")
            return self.text_result(f"{note['title']}\n{note['body']}")
        raise ValueError(f"unknown tool: {name}")


if __name__ == "__main__":
    serve(Handler, 3002)
