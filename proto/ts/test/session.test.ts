import { describe, expect, it } from "vitest";
import {
  Channel,
  CloseCode,
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
  send(data: Uint8Array) {
    const copy = data.slice();
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

async function connected(): Promise<{ s: Session; srv: Server; ws: MockSocket }> {
  const a = new MockSocket();
  const b = new MockSocket();
  a.peer = b;
  b.peer = a;
  const srv = new Server(b);
  const p = Session.connect(a, { client: "executor" });
  const hello = await srv.nextCtrl("hello");
  expect(hello).toEqual({ type: "hello", versions: [1], client: "executor" });
  srv.ctrl({ type: "welcome", version: 1 });
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

describe("runs", () => {
  const run = {
    run_id: "job-1",
    repo_url: "https://x/r",
    base_ref: "main",
    branch: "b",
    push: false,
    clone_token: "t",
    prompt: "p",
    timeout_secs: 60,
    callback_url: "http://cb",
    callback_token: "c",
  };

  it("run_start with a log channel", async () => {
    const { s, srv } = await connected();
    const p = s.runStart({ ...run, log: true });
    const m = await srv.nextCtrl("run_start");
    expect(m.log_channel).toBeTypeOf("number");
    srv.ctrl({ type: "run_accepted", run_id: "job-1", state: "running", existing: false });
    const r = await p;
    expect(r.accepted).toEqual({ run_id: "job-1", state: "running", existing: false });
    srv.send(dataJson(r.log!.id, { line: "hi" }));
    expect(await r.log!.readJson()).toEqual({ line: "hi" });
  });

  it("run_start without log sends log_channel null", async () => {
    const { s, srv } = await connected();
    const p = s.runStart(run);
    const m = await srv.nextCtrl("run_start");
    expect(m.log_channel).toBeNull();
    srv.ctrl({ type: "run_accepted", run_id: "job-1", state: "queued", existing: true });
    expect((await p).log).toBeUndefined();
  });

  it("run_status resolves from run_state; unknown_run rejects; pushes go to onCtrl", async () => {
    const { s, srv } = await connected();
    const pushed: Ctrl[] = [];
    s.onCtrl((m) => pushed.push(m));
    const p = s.runStatus("job-1");
    await srv.nextCtrl("run_status");
    srv.ctrl({ type: "run_state", run_id: "job-1", state: "succeeded", exit_code: 0, summary: "ok" });
    expect(await p).toMatchObject({ state: "succeeded", exit_code: 0 });

    const q = s.runStatus("nope");
    await srv.nextCtrl("run_status");
    srv.ctrl({ type: "error", code: "unknown_run", message: "no" });
    await expect(q).rejects.toMatchObject({ code: "remote", details: { remote: { code: "unknown_run" } } });

    srv.ctrl({ type: "run_state", run_id: "job-1", state: "failed" });
    srv.ctrl({ type: "credential_captured", kind: "claude_oauth" });
    await tick();
    expect(pushed.map((m) => m.type)).toEqual(["run_state", "credential_captured"]);
  });

  it("a rejected run_start frees its channel id", async () => {
    const { s, srv } = await connected();
    const p = s.runStart({ ...run, log: 50 });
    await srv.nextCtrl("run_start");
    srv.ctrl({ type: "error", code: "internal", message: "boom" });
    await expect(p).rejects.toBeInstanceOf(SessionError);
    await expect(s.runStart({ ...run, log: 0 })).rejects.toMatchObject({ code: "channel_in_use" });
    const p2 = s.runStart({ ...run, log: 50 });
    await srv.nextCtrl("run_start");
    srv.ctrl({ type: "run_accepted", run_id: "job-1", state: "queued", existing: false });
    expect((await p2).log!.id).toBe(50);
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
    const p = s.runStatus("x");
    await srv.nextCtrl("run_status");
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
