#!/usr/bin/env node
// A fake ACP agent for tests/steps.rs (newline-delimited JSON-RPC over stdio).
// The first word of a prompt selects the behaviour. Sessions are recorded in
// $FAKE_STATE so `session/load` works across "restarts".
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";

const state = process.env.FAKE_STATE;
mkdirSync(state, { recursive: true });
const send = (m) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...m }) + "\n");
const reply = (id, result) => send({ id, result });
const update = (sessionId, u) => send({ method: "session/update", params: { sessionId, update: u } });
const say = (sid, text) => update(sid, { sessionUpdate: "agent_message_chunk", content: { type: "text", text } });

const servers = new Map(); // sessionId -> mcpServers
let cancel = null;
let nextId = 1000;
const pending = new Map();

function mcpCall(server, calls) {
  return new Promise((resolve) => {
    const child = spawn(server.command, server.args, { stdio: ["pipe", "pipe", "inherit"] });
    const rl = createInterface({ input: child.stdout });
    const results = [];
    let i = 0;
    const next = () => {
      if (i >= calls.length) {
        child.stdin.end();
        resolve(results);
        return;
      }
      child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: i + 1, ...calls[i] }) + "\n");
    };
    rl.on("line", (line) => {
      results.push(JSON.parse(line));
      i++;
      next();
    });
    next();
  });
}

async function prompt(id, params) {
  const sid = params.sessionId;
  const text = params.prompt.map((b) => b.text ?? "").join("");
  const word = text.split(/\s+/)[0];
  if (word === "echo") {
    say(sid, "hello ");
    say(sid, "world");
    update(sid, { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "thinking" } });
    update(sid, { sessionUpdate: "tool_call", toolCallId: "t1", title: "Read", kind: "read", status: "pending" });
    const perm = nextId++;
    pending.set(perm, () => {});
    send({ id: perm, method: "session/request_permission", params: { sessionId: sid, toolCall: { toolCallId: "t1" },
      options: [{ optionId: "ok", kind: "allow_once" }, { optionId: "no", kind: "reject_once" }] } });
    update(sid, { sessionUpdate: "tool_call_update", toolCallId: "t1", status: "completed", rawOutput: "done" });
    say(sid, `secret=${process.env.API_KEY} gh=ghp_abcdefghijklmnop12 model=${process.env.ANTHROPIC_MODEL}`);
    reply(id, { stopReason: "end_turn", usage: { inputTokens: 10, outputTokens: 5 } });
  } else if (word === "submit") {
    const server = (servers.get(sid) ?? [])[0];
    if (!server) {
      say(sid, "no mcp server");
      return reply(id, { stopReason: "end_turn" });
    }
    const call = (args) => ({ method: "tools/call", params: { name: "submit_result", arguments: args } });
    const res = await mcpCall(server, [
      { method: "initialize", params: { protocolVersion: "2025-06-18" } },
      { method: "tools/list" },
      call({ verdict: "maybe" }),
      call({ verdict: "approve" }),
    ]);
    say(sid, `tools=${res[1].result.tools.map((t) => t.name).join(",")}`);
    say(sid, ` invalid=${res[2].result.isError} valid=${res[3].result.isError}`);
    reply(id, { stopReason: "end_turn" });
  } else if (word === "wait") {
    say(sid, "waiting");
    await new Promise((resolve) => {
      const t = setTimeout(resolve, 60000);
      cancel = () => {
        clearTimeout(t);
        resolve();
      };
    });
    reply(id, { stopReason: "cancelled" });
  } else if (word === "slow") {
    await new Promise((r) => setTimeout(r, 1500));
    say(sid, "slow done");
    reply(id, { stopReason: "end_turn" });
  } else if (word === "The") {
    say(sid, "resumed");
    reply(id, { stopReason: "end_turn" });
  } else {
    say(sid, `steered: ${text}`);
    reply(id, { stopReason: "end_turn" });
  }
}

createInterface({ input: process.stdin }).on("line", (line) => {
  const m = JSON.parse(line);
  if (m.method === undefined) {
    pending.get(m.id)?.(m);
    return;
  }
  switch (m.method) {
    case "initialize":
      return reply(m.id, { protocolVersion: 1, agentCapabilities: { loadSession: true }, agentInfo: { name: "fake" } });
    case "session/new": {
      const sid = `sess-${process.pid}`;
      servers.set(sid, m.params.mcpServers);
      writeFileSync(join(state, sid), JSON.stringify(m.params));
      return reply(m.id, { sessionId: sid });
    }
    case "session/load": {
      const sid = m.params.sessionId;
      if (!existsSync(join(state, sid))) return send({ id: m.id, error: { code: -32000, message: "no session" } });
      servers.set(sid, m.params.mcpServers);
      // History replay; must not land in the step's log again.
      update(sid, { sessionUpdate: "user_message_chunk", content: { type: "text", text: "REPLAYED" } });
      return reply(m.id, {});
    }
    case "session/prompt":
      return void prompt(m.id, m.params);
    case "session/cancel":
      return cancel?.();
    default:
      if (m.id !== undefined) send({ id: m.id, error: { code: -32601, message: "nope" } });
  }
});
