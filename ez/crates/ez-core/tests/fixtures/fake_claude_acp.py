#!/usr/bin/env python3
"""Fake claude-agent-acp for ez's claude-code driver tests.

Speaks newline JSON-RPC ACP on stdio. On session/new it checks the params ez must send; on each
session/prompt it calls ez's MCP tool server like Claude Code would, then reports what it saw as
agent text: `prompt=ok tools=Read,Write Write=ok Bash=error prompts=N`.
"""
import json
import os
import sys
import urllib.request

state = {"prompts": 0}


def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def reply(id_, result=None, error=None):
    msg = {"jsonrpc": "2.0", "id": id_}
    if error is not None:
        msg["error"] = {"code": -32602, "message": error}
    else:
        msg["result"] = result
    send(msg)


def update(update_):
    send({"jsonrpc": "2.0", "method": "session/update",
          "params": {"sessionId": "s1", "update": update_}})


def mcp(method, params, id_=1):
    body = json.dumps({"jsonrpc": "2.0", "id": id_, "method": method, "params": params}).encode()
    req = urllib.request.Request(state["url"], data=body, method="POST", headers={
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "Authorization": state["auth"],
    })
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.loads(r.read())["result"]


def check_session_new(p):
    if os.path.exists(os.path.join(p["cwd"], "FAIL_SESSION_NEW")):
        return "refusing session (test)"
    sp = p.get("systemPrompt", {})
    if sp.get("mode") != "override":
        return "systemPrompt must be override"
    text = "".join(b.get("text", "") for b in sp.get("content", []))
    opts = p.get("_meta", {}).get("claudeCode", {}).get("options", {})
    if opts.get("tools") != []:
        return "built-in tools must be disabled"
    if opts.get("allowedTools") != ["mcp__ez__*"]:
        return "allowedTools must be mcp__ez__*"
    if opts.get("toolAliases", {}).get("Write") != "mcp__ez__Write":
        return "missing Write alias"
    if opts.get("settingSources") != []:
        return "settingSources must be empty"
    if opts.get("model") != "sonnet":
        return "model not passed"
    servers = p.get("mcpServers", [])
    if len(servers) != 1 or servers[0].get("type") != "http" or servers[0].get("name") != "ez":
        return "expected one http MCP server named ez"
    headers = {h["name"]: h["value"] for h in servers[0].get("headers", [])}
    state["url"] = servers[0]["url"]
    state["auth"] = headers.get("Authorization", "")
    state["prompt_ok"] = "MODE-BUILD-PROMPT" in text
    state["cwd"] = p["cwd"]
    return None


def on_prompt(p):
    state["prompts"] += 1
    mcp("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                       "clientInfo": {"name": "fake-claude", "version": "0"}})
    tools = [t["name"] for t in mcp("tools/list", {}, 2)["tools"]]
    parts = ["prompt=" + ("ok" if state.get("prompt_ok") else "missing"),
             "tools=" + ",".join(tools)]
    if state["prompts"] == 1:
        calls = [("Write", {"file_path": "out.txt", "content": "hello from claude"}),
                 ("Bash", {"command": "echo nope"})]
        for i, (name, args) in enumerate(calls):
            r = mcp("tools/call", {"name": name, "arguments": args}, 3 + i)
            parts.append(name + "=" + ("error" if r.get("isError") else "ok"))
    parts.append("prompts=%d" % state["prompts"])
    update({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hmm"}})
    update({"sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": " ".join(parts)}})
    update({"sessionUpdate": "usage_update", "used": 1234, "size": 200000})
    return {"stopReason": "end_turn",
            "usage": {"inputTokens": 10, "outputTokens": 7, "cachedReadTokens": 90,
                      "cachedWriteTokens": 0, "totalTokens": 107}}


for line in sys.stdin:
    msg = json.loads(line)
    method, id_, params = msg.get("method"), msg.get("id"), msg.get("params", {})
    if method == "initialize":
        reply(id_, {"protocolVersion": 1, "agentCapabilities": {}})
    elif method == "session/new":
        err = check_session_new(params)
        if err:
            sys.stderr.write("fake: " + err + "\n")
            sys.stderr.flush()
            reply(id_, error=err)
        else:
            reply(id_, {"sessionId": "s1"})
    elif method == "session/prompt":
        reply(id_, on_prompt(params))
    elif method == "session/cancel":
        pass
    elif id_ is not None:
        reply(id_, error="unknown method " + str(method))
