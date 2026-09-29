"""MCP 3/5: weather — deterministic fake forecast (no external API)."""
from base import McpHandler, serve

FORECAST = {
    "madrid": "soleado, 24C, viento 10 km/h",
    "londres": "lluvia ligera, 14C, humedad 85%",
    "mexico": "despejado, 26C, probabilidad de lluvia 10%",
}


class Handler(McpHandler):
    SERVER_NAME = "weather"
    TOOLS = [
        {"name": "weather_get", "description": "Get fake weather for a city",
         "inputSchema": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}},
        {"name": "cities_list", "description": "List cities with data",
         "inputSchema": {"type": "object", "properties": {}}},
    ]
    RESOURCES = [{"uri": "weather://cities", "name": "cities", "description": "Cities with forecast data"}]
    PROMPTS = [{"name": "pack", "description": "What should I pack for this weather?"}]

    def handle_tool(self, name, args):
        if name == "cities_list":
            return self.text_result(", ".join(sorted(FORECAST)))
        if name == "weather_get":
            city = str(args["city"]).lower()
            if city not in FORECAST:
                raise ValueError(f"no data for '{args['city']}'. Try: {', '.join(sorted(FORECAST))}")
            return self.text_result(f"{args['city']}: {FORECAST[city]}")
        raise ValueError(f"unknown tool: {name}")


if __name__ == "__main__":
    serve(Handler, 3003)
