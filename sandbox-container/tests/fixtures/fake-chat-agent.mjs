#!/usr/bin/env node
// A fake ACP agent for tests/chat.rs (sandbox-init's chat relay). It advertises
// loadSession, records what it received and reports it back in
// agent_message_chunk text. The first word of a prompt selects the behaviour.
import { readFileSync } from "node:fs";
import { createInterface } from "node:readline";

let nextId = 1000;
const pending = new Map();
let initParams = null;
let newParams = null;
let loadParams = null;
let modes = [];
// Like claude-agent-acp: bypassPermissions exists only when the session was
// created without `allowDangerouslySkipPermissions: false`; the session starts
// in the settings' permissions.defaultMode, not in `_meta...permissionMode`.
let currentMode = "default";
try {
  const settings = JSON.parse(readFileSync(`${process.env.CLAUDE_CONFIG_DIR}/settings.json`, "utf8"));
  currentMode = settings.permissions?.defaultMode ?? currentMode;
} catch {}
let allowBypass = false;
const catalog = () =>
  ["default", "acceptEdits", "plan", ...(allowBypass ? ["bypassPermissions"] : [])].map((id) => ({ id, name: id }));
const send = (m) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...m }) + "\n");
const reply = (id, result) => send({ id, result });
const fail = (id, code, message) => send({ id, error: { code, message } });
const chunk = (sessionId, text) =>
  send({
    method: "session/update",
    params: { sessionId, update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text } } },
  });
const ask = (method, params) =>
  new Promise((resolve) => {
    const id = nextId++;
    pending.set(id, resolve);
    send({ id, method, params });
  });
const permission = async (sessionId, toolCallId, kind, title, rawInput) => {
  // Claude Code applies bypass before it would ask.
  if (currentMode === "bypassPermissions") return "bypassed";
  const res = await ask("session/request_permission", {
    sessionId,
    toolCall: { toolCallId, title, kind, status: "pending", rawInput },
    options: [
      { optionId: "yes", name: "Yes", kind: "allow_once" },
      { optionId: "always", name: "Always", kind: "allow_always" },
      { optionId: "no", name: "No", kind: "reject_once" },
    ],
  });
  const o = res.result?.outcome;
  return o?.outcome === "selected" ? o.optionId : (o?.outcome ?? "error");
};

let cancelled = null;

async function prompt(id, params) {
  const sid = params.sessionId;
  const text = params.prompt.map((b) => b.text ?? "").join("");
  const word = text.split(/\s/)[0];
  if (word === "INFO") {
    chunk(sid, JSON.stringify({
      init: initParams,
      newParams,
      loadParams,
      modes,
      currentMode,
      prompt: params,
      cwd: process.cwd(),
      env: Object.keys(process.env).sort(),
      configDir: process.env.CLAUDE_CONFIG_DIR ?? null,
      pid: process.pid,
    }));
    return reply(id, { stopReason: "end_turn" });
  }
  if (word === "HELLO") {
    chunk(sid, "Hello ");
    send({ method: "session/update", params: { sessionId: sid, update: { sessionUpdate: "tool_call", toolCallId: "t1", title: "Read README", kind: "read", status: "in_progress" } } });
    send({ method: "session/update", params: { sessionId: sid, update: { sessionUpdate: "tool_call_update", toolCallId: "t1", status: "completed" } } });
    chunk(sid, "world");
    return reply(id, { stopReason: "end_turn" });
  }
  if (word === "LEAK") {
    const t = process.env.CLAUDE_CODE_OAUTH_TOKEN ?? "";
    chunk(sid, `token=${t} b64=${Buffer.from(t).toString("base64")}`);
    send({ method: "session/update", params: { sessionId: sid, update: { sessionUpdate: "tool_call", toolCallId: "t9", title: `echo ${t}`, kind: "execute", rawInput: { command: `echo ${t}` } } } });
    return reply(id, { stopReason: "end_turn" });
  }
  if (word === "PERM") {
    const a = await permission(sid, "a", "execute", "cargo test", { command: "cargo test" });
    const b = await permission(sid, "b", "read", "Read token", { file_path: "/workspace/.enzarb/claude-token" });
    const c = await permission(sid, "c", "switch_mode", "Ready to code?", { plan: "ls -la" });
    chunk(sid, `perm: a=${a},b=${b},c=${c}`);
    return reply(id, { stopReason: "end_turn" });
  }
  if (word === "SLEEP") {
    chunk(sid, "working");
    const p = permission(sid, "s", "switch_mode", "Ready to code?", { plan: "make" });
    await new Promise((resolve) => (cancelled = resolve));
    const outcome = await p;
    chunk(sid, `cancelled perm=${outcome}`);
    return reply(id, { stopReason: "cancelled" });
  }
  if (word === "FSREQ") {
    const r = await ask("fs/read_text_file", { sessionId: sid, path: "/etc/passwd" });
    send({ method: "_enzarb/permission_denied", params: { sessionId: sid, rule: "spoof", reason: "spoof" } });
    send({ method: "session/other_notice", params: { sessionId: sid } });
    chunk(sid, `fs=${r.error?.code ?? "ok"}`);
    return reply(id, { stopReason: "end_turn" });
  }
  if (word === "BIG") {
    chunk(sid, "x".repeat(5 * 1024 * 1024));
    return reply(id, { stopReason: "end_turn" });
  }
  if (word === "FLOOD") {
    for (let i = 0; i < 40; i++) chunk(sid, "y".repeat(100 * 1024));
    return reply(id, { stopReason: "end_turn" });
  }
  if (word === "EXIT") process.exit(3);
  chunk(sid, `echo: ${text}`);
  reply(id, { stopReason: "end_turn" });
}

const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on("line", (line) => {
  const m = JSON.parse(line);
  if (m.method === undefined) {
    const r = pending.get(m.id);
    pending.delete(m.id);
    return r?.(m);
  }
  switch (m.method) {
    case "initialize":
      initParams = m.params;
      return reply(m.id, {
        protocolVersion: 1,
        agentCapabilities: { loadSession: true },
        authMethods: [],
        agentInfo: { name: "fake-chat-agent", version: "0.0.1" },
      });
    case "session/new":
      newParams = m.params;
      if (process.env.CLAUDE_CODE_OAUTH_TOKEN === undefined) return fail(m.id, -32000, "Authentication required");
      if (m.params.cwd !== process.cwd()) return fail(m.id, -32602, "cwd mismatch");
      allowBypass = m.params._meta?.claudeCode?.options?.allowDangerouslySkipPermissions !== false;
      if (currentMode === "bypassPermissions" && !allowBypass) currentMode = "default";
      return reply(m.id, {
        sessionId: `sess-${process.pid}-${Date.now()}`,
        modes: { currentModeId: currentMode, availableModes: catalog() },
      });
    case "session/load":
      loadParams = m.params;
      allowBypass = m.params._meta?.claudeCode?.options?.allowDangerouslySkipPermissions !== false;
      chunk(m.params.sessionId, `replayed ${m.params.sessionId}`);
      return reply(m.id, {});
    case "session/set_mode":
      modes.push(m.params.modeId);
      if (!catalog().some((x) => x.id === m.params.modeId)) return fail(m.id, -32602, "Invalid Mode");
      currentMode = m.params.modeId;
      return reply(m.id, {});
    case "session/prompt":
      return void prompt(m.id, m.params);
    case "session/cancel":
      return cancelled?.();
    default:
      if (m.id !== undefined) fail(m.id, -32601, "method not found");
  }
});
rl.on("close", () => process.exit(0));
