#!/usr/bin/env python3
"""Tiny MCP stdio server for tests: initialize, tools/list (paginated), tools/call."""
import json
import sys
import time

extra = int(sys.argv[1]) if len(sys.argv) > 1 else 0

TOOLS = [
    {"name": "echo", "description": "Echo text back",
     "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]},
     "annotations": {"readOnlyHint": True}},
    {"name": "fail", "description": "Always fails",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "mixed", "description": "Returns mixed content",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "slow", "description": "Sleeps",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "weird.name", "description": "Name needs sanitizing",
     "inputSchema": {"type": "object", "properties": {}}},
]
TOOLS += [{"name": f"extra{i}", "description": f"Extra {i}",
           "inputSchema": {"type": "object", "properties": {}}} for i in range(extra)]


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def call(name, args):
    if name == "echo":
        return {"content": [{"type": "text", "text": "echo: " + args.get("text", "")}]}
    if name == "fail":
        return {"content": [{"type": "text", "text": "boom"}], "isError": True}
    if name == "mixed":
        return {"content": [
            {"type": "text", "text": "hello"},
            {"type": "image", "data": "AAAA", "mimeType": "image/png"},
            {"type": "resource", "resource": {"uri": "file:///x.bin", "mimeType": "application/octet-stream", "blob": "AA=="}},
        ]}
    if name == "slow":
        time.sleep(30)
        return {"content": [{"type": "text", "text": "done"}]}
    return {"content": [{"type": "text", "text": "called " + name}]}


for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method, mid = msg.get("method"), msg.get("id")
    if mid is None:
        continue  # notification
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {
            "protocolVersion": msg["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fake", "version": "0.0.1"}}})
    elif method == "tools/list":
        cursor = (msg.get("params") or {}).get("cursor")
        # Two pages: first 3 tools, then the rest.
        if cursor is None:
            send({"jsonrpc": "2.0", "id": mid, "result": {"tools": TOOLS[:3], "nextCursor": "p2"}})
        else:
            send({"jsonrpc": "2.0", "id": mid, "result": {"tools": TOOLS[3:]}})
    elif method == "tools/call":
        p = msg["params"]
        send({"jsonrpc": "2.0", "id": mid, "result": call(p["name"], p.get("arguments") or {})})
    elif method == "ping":
        send({"jsonrpc": "2.0", "id": mid, "result": {}})
    else:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "no such method"}})
