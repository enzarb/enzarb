import { inspect } from "node:util";
import { describe, expect, it } from "vitest";
import {
  Secret,
  asCtrl,
  ByteChannel,
  Channel,
  CloseCode,
  OpenError,
  type Ctrl,
  type Frame,
  FrameType,
  INITIAL_WINDOW,
  Session,
  SessionError,
  type WebSocketLike,
  closeFrame,
  closeInfo,
  ctrlFrame,
  dataBin,
  dataJson,
  decodeFrame,
  encodeFrame,
  parseCtrl,
  parseJson,
  windowCredit,
  windowFrame,
} from "../src/index.js";

type Listener = (ev: any) => void;

/** A WebSocket test double; `peer` is the "server" end driven by the test. */
class MockSocket implements WebSocketLike {
  binaryType = "blob";
  readyState = 1;
  peer!: MockSocket;
  private listeners = new Map<string, Listener[]>();
  inbox: Uint8Array[] = [];
  private waiters: Array<() => void> = [];
  closedWith: number | undefined;

  addEventListener(type: string, l: Listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), l]);
  }
  private emit(type: string, ev: any) {
    for (const l of this.listeners.get(type) ?? []) l(ev);
  }
  send(data: BufferSource) {
    const copy = ArrayBuffer.isView(data)
      ? new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength))
      : new Uint8Array(data.slice(0));
    queueMicrotask(() => this.peer.deliver(copy.buffer));
  }
  deliver(data: unknown) {
    if (data instanceof ArrayBuffer) this.inbox.push(new Uint8Array(data));
    this.emit("message", { data });
    this.waiters.splice(0).forEach((w) => w());
  }
  close(code?: number) {
    this.closedWith = code;
    this.readyState = 3;
    queueMicrotask(() => {
      this.emit("close", {});
      this.peer.readyState = 3;
      this.peer.emit("close", {});
    });
  }
}

class Server {
  frames: Frame[] = [];
  private waiters: Array<() => void> = [];
  constructor(readonly ws: MockSocket) {
    ws.addEventListener("message", (ev) => {
      this.frames.push(decodeFrame(new Uint8Array(ev.data)));
      this.waiters.splice(0).forEach((w) => w());
    });
  }
  send(f: Frame) {
    this.ws.send(encodeFrame(f));
  }
  ctrl(m: Ctrl) {
    this.send(ctrlFrame(m));
  }
  /** Wait for (and consume) the next received frame matching `pred`. */
  async next(pred: (f: Frame) => boolean = () => true): Promise<Frame> {
    for (;;) {
      const i = this.frames.findIndex(pred);
      if (i >= 0) return this.frames.splice(i, 1)[0]!;
      await new Promise<void>((r) => this.waiters.push(r));
    }
  }
  async nextCtrl<T extends Ctrl["type"]>(type: T): Promise<Extract<Ctrl, { type: T }>> {
    const f = await this.next((f) => f.type === FrameType.Ctrl && parseCtrl(f).type === type);
    return parseCtrl(f) as any;
  }
}

async function connected(features?: unknown): Promise<{ s: Session; srv: Server; ws: MockSocket }> {
  const a = new MockSocket();
  const b = new MockSocket();
  a.peer = b;
  b.peer = a;
  const srv = new Server(b);
  const p = Session.connect(a, { client: "executor" });
  const hello = await srv.nextCtrl("hello");
  expect(hello).toEqual({ type: "hello", versions: [1], client: "executor" });
  srv.ctrl(features === undefined ? { type: "welcome", version: 1 } : ({ type: "welcome", version: 1, features } as Ctrl));
  return { s: await p, srv, ws: a };
}

async function openChannel(s: Session, srv: Server): Promise<Channel> {
  const p = s.openPty({ cols: 80, rows: 24 });
  const open = await srv.nextCtrl("open");
  srv.ctrl({ type: "opened", channel: open.channel });
  return p;
}

const tick = () => new Promise((r) => setTimeout(r, 20));

describe("handshake", () => {
  it("negotiates and sets binaryType", async () => {
    const { s, ws } = await connected();
    expect(s.version).toBe(1);
    expect(ws.binaryType).toBe("arraybuffer");
  });

  it("rejects when the server refuses", async () => {
    const a = new MockSocket();
    const b = new MockSocket();
    a.peer = b;
    b.peer = a;
    const srv = new Server(b);
    const p = Session.connect(a);
    await srv.nextCtrl("hello");
    srv.ctrl({ type: "error", code: "unsupported_version", message: "no" });
    await expect(p).rejects.toMatchObject({ code: "handshake" });
  });

  it("rejects when the first server frame is not welcome", async () => {
    const a = new MockSocket();
    const b = new MockSocket();
    a.peer = b;
    b.peer = a;
    const srv = new Server(b);
    const p = Session.connect(a);
    await srv.nextCtrl("hello");
    srv.ctrl({ type: "opened", channel: 1 });
    await expect(p).rejects.toMatchObject({ code: "handshake" });
  });

  it("times out", async () => {
    const a = new MockSocket();
    a.peer = new MockSocket();
    await expect(Session.connect(a, { handshakeTimeoutMs: 30 })).rejects.toMatchObject({ code: "handshake" });
  });
});

describe("channels", () => {
  it("opens, exchanges bytes, and handles open_error", async () => {
    const { s, srv } = await connected();
    const ch = await openChannel(s, srv);
    await ch.write("hi");
    const f = await srv.next((f) => f.type === FrameType.DataBin);
    expect(f.channel).toBe(ch.id);
    expect(new TextDecoder().decode(f.payload)).toBe("hi");
    srv.send(dataBin(ch.id, new TextEncoder().encode("yo")));
    const d = await ch.read();
    expect(d && new TextDecoder().decode(d.data)).toBe("yo");

    const p = s.openPty({ cols: 1, rows: 1, command: ["sh"] });
    const open = await srv.nextCtrl("open");
    expect(open.command).toEqual(["sh"]);
    srv.ctrl({ type: "open_error", channel: open.channel, message: "full" });
    await expect(p).rejects.toMatchObject({ code: "open_rejected" });
  });

  it("applies backpressure: writes stall at the window and resume on WINDOW", async () => {
    const { s, srv } = await connected();
    const ch = await openChannel(s, srv);
    const total = INITIAL_WINDOW * 2;
    let done = false;
    const w = ch.write(new Uint8Array(total).fill(7)).then(() => (done = true));
    await tick();
    let got = 0;
    while (srv.frames.some((f) => f.type === FrameType.DataBin)) {
      got += (await srv.next((f) => f.type === FrameType.DataBin)).payload.length;
    }
    expect(got).toBe(INITIAL_WINDOW);
    expect(done).toBe(false);
    expect(ch.credit).toBe(0);
    srv.send(windowFrame(ch.id, INITIAL_WINDOW));
    await w;
    await tick();
    while (srv.frames.some((f) => f.type === FrameType.DataBin)) {
      got += (await srv.next((f) => f.type === FrameType.DataBin)).payload.length;
    }
    expect(got).toBe(total);
  });

  it("a stalled channel does not block another", async () => {
    const { s, srv } = await connected();
    const a = await openChannel(s, srv);
    const b = await openChannel(s, srv);
    void a.write(new Uint8Array(INITIAL_WINDOW * 2));
    await tick();
    await b.write("still works");
    const f = await srv.next((f) => f.type === FrameType.DataBin && f.channel === b.id);
    expect(new TextDecoder().decode(f.payload)).toBe("still works");
  });

  it("returns credit as data is read", async () => {
    const { s, srv } = await connected();
    const ch = await openChannel(s, srv);
    for (let i = 0; i < 12; i++) srv.send(dataBin(ch.id, new Uint8Array(16 * 1024)));
    await tick();
    expect(srv.frames.some((f) => f.type === FrameType.Window)).toBe(false);
    for (let i = 0; i < 12; i++) await ch.read();
    const w = await srv.next((f) => f.type === FrameType.Window);
    expect(w.channel).toBe(ch.id);
    expect(windowCredit(w)).toBe(INITIAL_WINDOW / 2);
  });

  it("json frames and async iteration; peer close ends the stream", async () => {
    const { s, srv } = await connected();
    const ch = await openChannel(s, srv);
    await ch.writeJson({ a: 1 });
    const f = await srv.next((f) => f.type === FrameType.DataJson);
    expect(parseJson(f.payload)).toEqual({ a: 1 });
    srv.send(dataJson(ch.id, { line: 1 }));
    srv.send(dataBin(ch.id, new Uint8Array([1])));
    srv.send(closeFrame(ch.id, 0, "exit 0"));
    const seen = [];
    for await (const d of ch) seen.push(d.kind);
    expect(seen).toEqual(["json", "bin"]);
    expect(ch.closeInfo).toEqual({ code: 0, reason: "exit 0" });
    // Half-close: we can still send, then close ourselves.
    await ch.write("x");
    await ch.close();
    const c = await srv.next((f) => f.type === FrameType.Close);
    expect(closeInfo(c)).toEqual({ code: 0, reason: "" });
    await expect(ch.write("y")).rejects.toMatchObject({ code: "channel_closed" });
  });

  it("answers data for unknown channels with an error", async () => {
    const { srv } = await connected();
    srv.send(dataBin(99, new Uint8Array([1])));
    const e = await srv.nextCtrl("error");
    expect(e.code).toBe("unknown_channel");
  });

  it("aborts a channel on credit overrun", async () => {
    const { s, srv } = await connected();
    const ch = await openChannel(s, srv);
    for (let i = 0; i < 17; i++) srv.send(dataBin(ch.id, new Uint8Array(16 * 1024)));
    const e = await srv.nextCtrl("error");
    expect(e.code).toBe("flow_control");
    const c = await srv.next((f) => f.type === FrameType.Close);
    expect(closeInfo(c).code).toBe(CloseCode.ProtocolError);
    await expect(ch.read()).rejects.toMatchObject({ code: "channel_aborted" });
    expect(s.isClosed).toBe(false);
  });
});

describe("ctrl", () => {
  it("step frames from the server go to onCtrl", async () => {
    const { s, srv } = await connected();
    const pushed: Ctrl[] = [];
    s.onCtrl((m) => pushed.push(m));
    srv.ctrl({ type: "step_accepted", run_id: "r", state: "running", existing: false });
    srv.ctrl({ type: "session_event", run_id: "r", seq: 1, at: "2026-10-16T00:00:00Z", kind: "stdout", payload: { text: "hi" } });
    srv.ctrl({ type: "step_state", run_id: "r", state: "finished", result: { run_id: "r", exit: "success" } });
    srv.ctrl({ type: "step_finished", run_id: "r", exit: "success" });
    srv.ctrl({ type: "credential_captured", kind: "claude_oauth" });
    await tick();
    expect(pushed.map((m) => m.type)).toEqual(["step_accepted", "session_event", "step_state", "step_finished", "credential_captured"]);
  });

  it("step_start tokens are wrapped in Secrets when parsed", () => {
    const m = asCtrl({
      type: "step_start",
      run_id: "r",
      attempt: 1,
      task_id: "t",
      workspaces: [{ repo: "o/r", url: "u", mode: "write", branch: "b", base: "main", token: "ghs_ws_secret" }],
      exec: { kind: "command", run: "true" },
      env: { API_KEY: "env_secret" },
      github_token: "ghs_gh_secret",
    });
    const d = inspect(m, { depth: 10 }) + JSON.stringify(m);
    for (const secret of ["ghs_ws_secret", "env_secret", "ghs_gh_secret"]) expect(d).not.toContain(secret);
    if (m.type !== "step_start") throw new Error("type");
    expect((m.workspaces[0]!.token as Secret).expose()).toBe("ghs_ws_secret");
    expect((m.env!.API_KEY as Secret).expose()).toBe("env_secret");
  });

  it("a stats reply is delivered to onCtrl", async () => {
    const { s, srv } = await connected();
    const pushed: Ctrl[] = [];
    s.onCtrl((m) => pushed.push(m));
    srv.ctrl({ type: "stats", ts_ms: 5, memory_bytes: 7, memory_limit_bytes: 9 });
    await tick();
    expect(pushed).toEqual([{ type: "stats", ts_ms: 5, memory_bytes: 7, memory_limit_bytes: 9 }]);
  });

  it("credentialStatus resolves from credential_state; error rejects", async () => {
    const { s, srv } = await connected();
    const p = s.credentialStatus();
    await srv.nextCtrl("credential_status");
    srv.ctrl({ type: "credential_state", kind: "claude_oauth", configured: true });
    expect(await p).toEqual({ kind: "claude_oauth", configured: true });

    const q = s.credentialStatus();
    await srv.nextCtrl("credential_status");
    srv.ctrl({ type: "error", code: "unimplemented", message: "no" });
    await expect(q).rejects.toMatchObject({ code: "remote" });

    const pushed: Ctrl[] = [];
    s.onCtrl((m) => pushed.push(m));
    srv.ctrl({ type: "credential_state", kind: "claude_oauth", configured: false });
    await tick();
    expect(pushed.map((m) => m.type)).toEqual(["credential_state"]);
  });

  it("credential_state.scope reaches credentialStatus() and unsolicited events", async () => {
    const { s, srv } = await connected();
    const p = s.credentialStatus();
    await srv.nextCtrl("credential_status");
    srv.ctrl({ type: "credential_state", kind: "claude_oauth", configured: true, scope: "user" });
    expect(await p).toEqual({ kind: "claude_oauth", configured: true, scope: "user" });

    const pushed: Ctrl[] = [];
    s.onCtrl((m) => pushed.push(m));
    srv.ctrl({ type: "credential_state", kind: "claude_oauth", configured: true, scope: "user" });
    await tick();
    expect(pushed[0]).toMatchObject({ type: "credential_state", scope: "user" });
  });

  it("credentialClear sends credential_clear", async () => {
    const { s, srv } = await connected();
    s.credentialClear("claude_oauth");
    expect(await srv.nextCtrl("credential_clear")).toEqual({ type: "credential_clear", kind: "claude_oauth" });
  });

  it("an old peer's unknown_message reply to credential_clear leaves the session alive", async () => {
    const { s, srv } = await connected();
    const pushed: Ctrl[] = [];
    s.onCtrl((m) => pushed.push(m));
    s.credentialClear();
    await srv.nextCtrl("credential_clear");
    srv.ctrl({ type: "error", code: "unknown_message", message: "unknown type credential_clear" });
    await tick();
    const p = s.credentialStatus();
    await srv.nextCtrl("credential_status");
    srv.ctrl({ type: "credential_state", kind: "claude_oauth", configured: false });
    expect(await p).toMatchObject({ configured: false });
    expect(pushed.map((m) => m.type)).toEqual(["error"]);
  });

  it("credential_captured token/legacy reach the subscriber, redacted when formatted", async () => {
    const { s, srv } = await connected();
    const pushed: Ctrl[] = [];
    s.onCtrl((m) => pushed.push(m));
    srv.ctrl({ type: "credential_captured", kind: "claude_oauth", token: "sk-secret-cap", legacy: true });
    await tick();
    const m = pushed[0]!;
    if (m.type !== "credential_captured") throw new Error("wrong type");
    expect(m.legacy).toBe(true);
    expect(m.token).toBeInstanceOf(Secret);
    expect((m.token as Secret).expose()).toBe("sk-secret-cap");
    for (const text of [inspect(m, { depth: 5 }), String(m.token), JSON.stringify(m), `${m.token}`]) {
      expect(text).not.toContain("sk-secret-cap");
    }
  });

  it("claude_token is sent as a plain string on open, and never leaks via inspect", async () => {
    const { s, srv } = await connected();
    const p = s.openAcp({ claudeToken: "sk-secret-open" });
    const f = await srv.next((f) => f.type === FrameType.Ctrl && parseCtrl(f).type === "open");
    expect(JSON.parse(new TextDecoder().decode(f.payload)).claude_token).toBe("sk-secret-open");
    const open = parseCtrl(f);
    expect(inspect(open, { depth: 5 })).not.toContain("sk-secret-open");
    expect(String(open.type === "open" ? open.claude_token : "")).not.toContain("sk-secret-open");
    srv.ctrl({ type: "opened", channel: (open as { channel: number }).channel });
    await p;
  });

  it("a DOM WebSocket is assignable to WebSocketLike without a cast", () => {
    // Type-level: fails `pnpm run check` if the DOM lib's WebSocket stops fitting.
    const accepts = (_: WebSocketLike) => {};
    const dom = null as unknown as WebSocket;
    accepts(dom);
    const ctor: new (url: string) => WebSocketLike = WebSocket;
    expect(ctor).toBeDefined();
  });

  it("attributes unimplemented errors to the oldest pending request", async () => {
    const { s, srv } = await connected();
    const p = s.credentialStatus();
    await srv.nextCtrl("credential_status");
    srv.ctrl({ type: "error", code: "unimplemented", message: "no" });
    await expect(p).rejects.toMatchObject({ code: "remote", details: { remote: { code: "unimplemented" } } });
  });
});

describe("shutdown", () => {
  it("unknown ctrl types get unknown_message", async () => {
    const { srv } = await connected();
    srv.send(ctrlFrame({ type: "teleport" } as any));
    expect((await srv.nextCtrl("error")).code).toBe("unknown_message");
  });

  it("a text message is fatal", async () => {
    const { s, ws } = await connected();
    ws.deliver("hello");
    const end = await s.closed;
    expect(end.kind).toBe("protocol");
  });

  it("a bad frame is fatal and fails pending work", async () => {
    const { s, srv } = await connected();
    const p = s.credentialStatus();
    await srv.nextCtrl("credential_status");
    srv.ws.send(new Uint8Array([9, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
    await expect(p).rejects.toMatchObject({ code: "ended" });
    expect((await s.closed).kind).toBe("protocol");
    await expect(s.openPty({ cols: 1, rows: 1 })).rejects.toMatchObject({ code: "ended" });
  });

  it("close() sends CLOSE on open channels and fails them", async () => {
    const { s, srv, ws } = await connected();
    const ch = await openChannel(s, srv);
    const end = await s.close();
    expect(end.kind).toBe("closed");
    expect(ws.closedWith).toBe(1000);
    const c = await srv.next((f) => f.type === FrameType.Close);
    expect(closeInfo(c).code).toBe(CloseCode.GoingAway);
    await expect(ch.read()).rejects.toMatchObject({ code: "ended" });
    await expect(s.openPty({ cols: 1, rows: 1 })).rejects.toBeInstanceOf(SessionError);
  });

  it("peer disconnect ends the session", async () => {
    const { s, srv } = await connected();
    const ch = await openChannel(s, srv);
    srv.ws.close();
    expect((await s.closed).kind).toBe("peer_closed");
    await expect(ch.write("x")).rejects.toMatchObject({ code: "ended" });
  });
});

describe("features", () => {
  it("is empty when welcome has none", async () => {
    const { s } = await connected();
    expect(s.features.size).toBe(0);
  });

  it("exposes advertised features, tolerating unknown ones", async () => {
    const { s } = await connected(["acp", "future_thing"]);
    expect(s.features.has("acp")).toBe(true);
    expect(s.features.has("future_thing")).toBe(true);
  });

  it("bounds hostile feature lists", async () => {
    const many = Array.from({ length: 500 }, (_, i) => `f${i}`);
    const { s } = await connected(["", "x".repeat(10_000), 7, null, ...many]);
    expect(s.features.size).toBe(64);
    expect(s.features.has("")).toBe(false);
    const { s: s2 } = await connected("not-an-array");
    expect(s2.features.size).toBe(0);
  });

  it("encodes no features key on hello/old paths", async () => {
    const { s } = await connected(null);
    expect(s.features.size).toBe(0);
  });
});

describe("openAcp", () => {
  async function openAcp(s: Session, srv: Server, opts?: { repo?: string; resume?: string }) {
    const p = s.openAcp(opts);
    const open = await srv.nextCtrl("open");
    srv.ctrl({ type: "opened", channel: open.channel });
    return { bc: await p, open };
  }

  it("sends kind acp with zero size and omits unset repo/resume", async () => {
    const { s, srv } = await connected(["acp"]);
    const { open } = await openAcp(s, srv);
    expect(open).toMatchObject({ kind: "acp", cols: 0, rows: 0 });
    expect("repo" in open).toBe(false);
    expect("resume" in open).toBe(false);
    expect("command" in open).toBe(false);
    const { open: o2 } = await openAcp(s, srv, { repo: "https://github.com/enzarb/x", resume: "sess_1" });
    expect(o2).toMatchObject({ repo: "https://github.com/enzarb/x", resume: "sess_1" });
  });

  it("surfaces open_error code as OpenError.openCode, and tolerates a missing code", async () => {
    const { s, srv } = await connected(["acp"]);
    let p = s.openAcp();
    let open = await srv.nextCtrl("open");
    srv.ctrl({ type: "open_error", channel: open.channel, message: "full", code: "chat_limit" });
    let e = await p.catch((x) => x);
    expect(e).toBeInstanceOf(OpenError);
    expect(e).toMatchObject({ code: "open_rejected", openCode: "chat_limit" });
    p = s.openAcp();
    open = await srv.nextCtrl("open");
    srv.ctrl({ type: "open_error", channel: open.channel, message: "old server" });
    e = await p.catch((x) => x);
    expect(e).toBeInstanceOf(OpenError);
    expect(e.openCode).toBeUndefined();
    // pty rejections are OpenErrors too (subclass of SessionError, same code)
    const q = s.openPty({ cols: 1, rows: 1 });
    open = await srv.nextCtrl("open");
    srv.ctrl({ type: "open_error", channel: open.channel, message: "x", code: "weird" });
    await expect(q).rejects.toMatchObject({ code: "open_rejected", openCode: "weird" });
  });

  it("refuses oversized repo/resume locally without sending anything", async () => {
    const { s, srv } = await connected(["acp"]);
    await expect(s.openAcp({ repo: "r".repeat(2049) })).rejects.toMatchObject({ openCode: "invalid_request" });
    await expect(s.openAcp({ resume: "é".repeat(200) })).rejects.toMatchObject({ openCode: "invalid_request" });
    await tick();
    expect(srv.frames.filter((f) => f.type === FrameType.Ctrl && parseCtrl(f).type === "open")).toHaveLength(0);
  });

  it("writable: chunks arrive in order and stall at the window until credit arrives", async () => {
    const { s, srv } = await connected(["acp"]);
    const { bc } = await openAcp(s, srv);
    const w = bc.writable.getWriter();
    const total = INITIAL_WINDOW * 2;
    const a = new Uint8Array(total / 2).fill(1);
    const b = new Uint8Array(total / 2).fill(2);
    let done = false;
    const p = (async () => {
      await w.write(a);
      await w.write(b);
      done = true;
    })();
    await tick();
    const take = async () => {
      const got: number[] = [];
      while (srv.frames.some((f) => f.type === FrameType.DataBin)) {
        got.push(...(await srv.next((f) => f.type === FrameType.DataBin)).payload);
      }
      return got;
    };
    const first = await take();
    expect(first.length).toBe(INITIAL_WINDOW);
    expect(done).toBe(false);
    srv.send(windowFrame(bc.id, INITIAL_WINDOW));
    await p;
    await tick();
    const rest = await take();
    const all = [...first, ...rest];
    expect(all.length).toBe(total);
    expect(all.slice(0, total / 2).every((x) => x === 1)).toBe(true);
    expect(all.slice(total / 2).every((x) => x === 2)).toBe(true);
    // closing the writable half-closes with a normal CLOSE
    await w.close();
    const c = await srv.next((f) => f.type === FrameType.Close);
    expect(closeInfo(c)).toEqual({ code: CloseCode.Normal, reason: "" });
  });

  it("writable rejects non-Uint8Array chunks", async () => {
    const { s, srv } = await connected(["acp"]);
    const { bc } = await openAcp(s, srv);
    const w = bc.writable.getWriter();
    await expect(w.write("text" as unknown as Uint8Array)).rejects.toBeInstanceOf(TypeError);
  });

  it("readable: delivers bytes in order and only returns credit as the consumer pulls", async () => {
    const { s, srv } = await connected(["acp"]);
    const { bc } = await openAcp(s, srv);
    const windows = () => srv.frames.filter((f) => f.type === FrameType.Window).length;
    // The peer fills the whole window; nothing is read yet.
    const chunk = new Uint8Array(16 * 1024);
    const n = INITIAL_WINDOW / chunk.length;
    for (let i = 0; i < n; i++) srv.send(dataBin(bc.id, chunk.fill(i + 1).slice()));
    await tick();
    expect(windows()).toBe(0); // no consumer, no credit: backpressure reaches the peer
    const r = bc.readable.getReader();
    const seen: number[] = [];
    for (let i = 0; i < n; i++) {
      const { value, done } = await r.read();
      expect(done).toBe(false);
      seen.push(value![0]!);
    }
    expect(seen).toEqual(Array.from({ length: n }, (_, i) => i + 1));
    await tick();
    expect(windows()).toBeGreaterThan(0);
    // peer CLOSE ends the stream and resolves `closed` with the reason
    srv.send(closeFrame(bc.id, CloseCode.Normal, "superseded"));
    expect((await r.read()).done).toBe(true);
    await expect(bc.closed).resolves.toEqual({ code: 0, reason: "superseded" });
  });

  it("round-trips 2 MiB each way under flow control with an echo peer", async () => {
    const { s, srv } = await connected(["acp"]);
    const { bc } = await openAcp(s, srv);
    const size = 2 * 1024 * 1024;
    const payload = new Uint8Array(size);
    for (let i = 0; i < size; i++) payload[i] = (i * 31) & 0xff;
    // Server: echo every data frame, granting credit like a real receiver.
    let rx = 0;
    let granted = 0;
    let credit = INITIAL_WINDOW; // what the client currently lets the server send
    const pump = (async () => {
      while (rx < size) {
        const f = await srv.next((f) => f.type === FrameType.DataBin);
        while (credit < f.payload.length) {
          credit += windowCredit(await srv.next((f) => f.type === FrameType.Window));
        }
        credit -= f.payload.length;
        srv.send(dataBin(bc.id, f.payload));
        rx += f.payload.length;
        granted += f.payload.length;
        if (granted >= INITIAL_WINDOW / 2) {
          srv.send(windowFrame(bc.id, granted));
          granted = 0;
        }
      }
      srv.send(closeFrame(bc.id));
    })();
    const w = bc.writable.getWriter();
    const writing = (async () => {
      await w.write(payload);
      await w.close();
    })();
    const out = new Uint8Array(size);
    let off = 0;
    for await (const part of bc.readable as unknown as AsyncIterable<Uint8Array>) {
      out.set(part, off);
      off += part.length;
    }
    await Promise.all([pump, writing]);
    expect(off).toBe(size);
    expect(Buffer.compare(out, payload)).toBe(0); // toEqual on 2 MiB is very slow
  });

  it("closed resolves on session end, and streams error", async () => {
    const { s, srv } = await connected(["acp"]);
    const { bc } = await openAcp(s, srv);
    const r = bc.readable.getReader();
    const pending = r.read().catch((e) => e);
    await s.close();
    expect((await bc.closed).code).toBeNull();
    expect(await pending).toBeInstanceOf(SessionError);
  });

  it("cancelling the readable closes the channel and keeps returning credit", async () => {
    const { s, srv } = await connected(["acp"]);
    const { bc } = await openAcp(s, srv);
    await bc.readable.cancel();
    const c = await srv.next((f) => f.type === FrameType.Close);
    expect(closeInfo(c).code).toBe(CloseCode.Cancelled);
    const n = INITIAL_WINDOW / 16384;
    for (let i = 0; i < n; i++) srv.send(dataBin(bc.id, new Uint8Array(16384)));
    await tick();
    expect(srv.frames.some((f) => f.type === FrameType.Window)).toBe(true);
    expect(bc).toBeInstanceOf(ByteChannel);
  });

  it("decodes unknown kind / code / old shapes in CTRL", () => {
    const f = ctrlFrame({ type: "open", channel: 1, kind: "quic", cols: 0, rows: 0 });
    expect(parseCtrl(f)).toMatchObject({ kind: "quic" });
    const g = ctrlFrame({ type: "open_error", channel: 1, message: "m", code: "brand_new" });
    expect(parseCtrl(g)).toMatchObject({ code: "brand_new" });
  });
});
