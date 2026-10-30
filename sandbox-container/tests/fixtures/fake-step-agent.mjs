#!/usr/bin/env node
// A fake ACP agent for tests/steps.rs (newline-delimited JSON-RPC over stdio).
// The first word of a prompt selects the behaviour. Sessions are recorded in
// $FAKE_STATE so `session/load` works across "restarts".
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
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

const cwds = new Map(); // sessionId -> cwd
// ez-like modes: the `[modes.<id>]` tables of $EZ_CONFIG_DIR/modes.toml (when set).
function ezModes() {
  const dir = process.env.EZ_CONFIG_DIR;
  if (!dir) return undefined;
  const text = readFileSync(join(dir, "modes.toml"), "utf8");
  const ids = [...text.matchAll(/^\[modes\.([A-Za-z0-9_-]+)\]/gm)].map((m) => m[1]);
  return { currentModeId: ids[0], availableModes: ids.map((id) => ({ id, name: id })) };
}
const modes = new Map(); // sessionId -> first word of its first prompt

async function prompt(id, params) {
  const sid = params.sessionId;
  const text = params.prompt.map((b) => b.text ?? "").join("");
  let word = text.split(/\s+/)[0];
  if (word === "Your") {
    // The continuation prompt: behave per the mode of the first prompt.
    word = `${modes.get(sid)}-continued`;
  } else if (!modes.has(sid)) {
    modes.set(sid, word);
  }
  if (word === "bg" || word === "twice") {
    // Starts background work and ends the turn without a result.
    update(sid, { sessionUpdate: "tool_call", toolCallId: "bg1", title: "Bash", kind: "execute", status: "pending" });
    say(sid, "waiting for it");
    reply(id, { stopReason: "end_turn", usage: { inputTokens: 3, outputTokens: 1 } });
  } else if (word === "bg-continued") {
    update(sid, { sessionUpdate: "tool_call", toolCallId: "bg2", title: "submit_result", kind: "other", status: "pending" });
    const server = (servers.get(sid) ?? [])[0];
    await mcpCall(server, [
      { method: "initialize", params: { protocolVersion: "2025-06-18" } },
      { method: "tools/call", params: { name: "submit_result", arguments: { verdict: "approve" } } },
    ]);
    reply(id, { stopReason: "end_turn", usage: { inputTokens: 4, outputTokens: 2 } });
  } else if (word === "quiet" || word === "twice-continued") {
    say(sid, "nothing to do");
    reply(id, { stopReason: "end_turn" });
  } else if (word === "echo") {
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
  } else if (word === "ezwork") {
    // Writes a file in the session's cwd; the file name is the second word.
    const name = text.split(/\s+/)[1] ?? "ez-out.txt";
    writeFileSync(join(cwds.get(sid), name), `by ${process.env.EZ_CONFIG_DIR}\n`);
    writeFileSync(
      join(state, "ezenv.json"),
      JSON.stringify({ config: process.env.EZ_CONFIG_DIR, sessions: process.env.EZ_SESSIONS_DIR, unattended: process.env.EZ_UNATTENDED, cwd: cwds.get(sid), gh: process.env.GH_TOKEN ?? null }),
    );
    say(sid, "wrote it");
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
  } else if (word === "enospc") {
    say(sid, "writing");
    send({ id, error: { code: -32000, message: "write error: No space left on device" } });
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
      cwds.set(sid, m.params.cwd);
      writeFileSync(join(state, sid), JSON.stringify(m.params));
      return reply(m.id, { sessionId: sid, modes: ezModes() });
    }
    case "session/load": {
      const sid = m.params.sessionId;
      if (!existsSync(join(state, sid))) return send({ id: m.id, error: { code: -32000, message: "no session" } });
      servers.set(sid, m.params.mcpServers);
      // History replay; must not land in the step's log again.
      update(sid, { sessionUpdate: "user_message_chunk", content: { type: "text", text: "REPLAYED" } });
      return reply(m.id, {});
    }
    case "session/set_mode": {
      const ms = ezModes();
      if (!ms?.availableModes.some((x) => x.id === m.params.modeId)) {
        return send({ id: m.id, error: { code: -32602, message: `no mode ${m.params.modeId}` } });
      }
      writeFileSync(join(state, "mode-set"), m.params.modeId);
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
