import { Channel, type ChannelHost } from "./channel.js";
import {
  type ClientKind,
  type Ctrl,
  ErrorCode,
  type RunPhase,
  type RunStartFields,
  UnknownCtrlType,
  parseCtrl,
} from "./ctrl.js";
import { SessionError, type SessionEnd } from "./errors.js";
import {
  CloseCode,
  type Frame,
  FrameError,
  FrameType,
  closeFrame,
  closeInfo,
  ctrlFrame,
  decodeFrame,
  encodeFrame,
  windowCredit,
} from "./frame.js";

/** The subset of the WebSocket API the session uses (browser, Node 22+, `ws`-compatible shims). */
export interface WebSocketLike {
  binaryType: string;
  readyState: number;
  send(data: Uint8Array): void;
  close(code?: number, reason?: string): void;
  addEventListener(type: string, listener: (ev: any) => void): void;
}

export interface ConnectOptions {
  /** Defaults to "ui". */
  client?: ClientKind;
  /** Versions to offer. Defaults to [1]. */
  versions?: number[];
  /** WebSocket constructor for string URLs. Defaults to `globalThis.WebSocket`. */
  WebSocket?: new (url: string) => WebSocketLike;
  /** Give up if `welcome` does not arrive in time. Default 10000; 0 disables. */
  handshakeTimeoutMs?: number;
}

/** `run_accepted` as seen by the client. */
export interface RunAccepted {
  run_id: string;
  state: RunPhase;
  existing: boolean;
}

/** `run_state` as seen by the client. */
export interface RunInfo {
  run_id: string;
  state: RunPhase;
  exit_code?: number | null;
  summary?: string | null;
}

export type RunStartRequest = Omit<RunStartFields, "log_channel"> & {
  /** `true` (allocate a transcript channel), a specific channel id, or omit/`false` for none. */
  log?: boolean | number;
};

export interface RunStarted {
  accepted: RunAccepted;
  /** The transcript channel (DATA_JSON lines) when requested. */
  log?: Channel;
}

type CtrlListener = (msg: Ctrl) => void;

type Pending =
  | { kind: "run_start"; run_id: string; log?: Channel; resolve: (r: RunStarted) => void; reject: (e: unknown) => void }
  | { kind: "run_status"; run_id: string; resolve: (r: RunInfo) => void; reject: (e: unknown) => void };

const OPEN = 1;
const ATTRIBUTABLE = new Set<string>([
  ErrorCode.UnknownRun,
  ErrorCode.Internal,
  ErrorCode.Unauthorized,
  ErrorCode.UnknownMessage,
  ErrorCode.Unimplemented,
]);

/**
 * Client side of the mux protocol over a WebSocket (browser `ui`, also usable from Node).
 *
 * ```ts
 * const s = await Session.connect(`wss://host/sandbox/v1/projects/${p}/mux?token=${jwt}`);
 * const pty = await s.openPty({ cols: 80, rows: 24 });
 * await pty.write("ls\n");
 * for await (const d of pty) term.write(d.data);   // credit is returned as you iterate
 * ```
 */
export class Session implements ChannelHost {
  /** Negotiated protocol version (valid once `connect` resolved). */
  version = 0;

  private channels = new Map<number, Channel>();
  private reserved = new Set<number>();
  private pendingOpen = new Map<number, { ch: Channel; resolve: (c: Channel) => void; reject: (e: unknown) => void }>();
  private pending: Pending[] = [];
  private listeners = new Set<CtrlListener>();
  private nextId = 0;
  private end: SessionEnd | null = null;
  private resolveClosed!: (e: SessionEnd) => void;
  private handshake: { resolve: () => void; reject: (e: unknown) => void; timer?: ReturnType<typeof setTimeout> } | null;

  /** Resolves when the session ends, for any reason. */
  readonly closed: Promise<SessionEnd>;

  private constructor(
    private ws: WebSocketLike,
    private opts: ConnectOptions,
    ready: { resolve: () => void; reject: (e: unknown) => void },
  ) {
    this.closed = new Promise((r) => (this.resolveClosed = r));
    this.handshake = ready;
    ws.binaryType = "arraybuffer";
    ws.addEventListener("message", (ev) => this.onMessage(ev.data));
    ws.addEventListener("close", () => this.finish({ kind: this.end ? this.end.kind : "peer_closed" }));
    ws.addEventListener("error", () => this.finish({ kind: "transport", reason: "websocket error" }));
    const sendHello = () =>
      this.sendCtrl({ type: "hello", versions: opts.versions ?? [1], client: opts.client ?? "ui" });
    if (ws.readyState === OPEN) sendHello();
    else ws.addEventListener("open", sendHello);
    const ms = opts.handshakeTimeoutMs ?? 10_000;
    if (ms > 0) {
      this.handshake.timer = setTimeout(() => this.fatalHandshake("handshake timed out"), ms);
    }
  }

  /** Open a websocket (or use the one given), perform the handshake, resolve when `welcome` arrived. */
  static async connect(target: string | URL | WebSocketLike, opts: ConnectOptions = {}): Promise<Session> {
    let ws: WebSocketLike;
    if (typeof target === "string" || target instanceof URL) {
      const Ctor = opts.WebSocket ?? (globalThis as { WebSocket?: new (u: string) => WebSocketLike }).WebSocket;
      if (!Ctor) throw new Error("no WebSocket implementation available; pass opts.WebSocket");
      ws = new Ctor(String(target));
    } else {
      ws = target;
    }
    let ready!: { resolve: () => void; reject: (e: unknown) => void };
    const p = new Promise<void>((resolve, reject) => (ready = { resolve, reject }));
    const s = new Session(ws, opts, ready);
    await p;
    return s;
  }

  // ---- lifecycle ----

  get isClosed(): boolean {
    return this.end !== null;
  }

  /** Subscribe to unsolicited CTRL messages (pushed `run_state`, `credential_captured`, unattributed `error`). */
  onCtrl(listener: CtrlListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Close gracefully: CLOSE (going away) on open channels, then the websocket. */
  async close(): Promise<SessionEnd> {
    if (!this.end) {
      for (const ch of [...this.channels.values()]) {
        if (!ch.isLocalClosed) void ch.close(CloseCode.GoingAway, "going away");
      }
      await Promise.resolve(); // let queued CLOSEs flush before the socket goes
      this.end = { kind: "closed" };
      try {
        this.ws.close(1000);
      } catch {
        /* already closing */
      }
      this.finish(this.end);
    }
    return this.closed;
  }

  private finish(end: SessionEnd): void {
    if (this.resolved) return;
    this.resolved = true;
    this.end ??= end;
    const e = this.end;
    if (this.handshake) {
      clearTimeout(this.handshake.timer);
      this.handshake.reject(new SessionError("handshake", `connection ended before welcome: ${e.kind}`, { end: e }));
      this.handshake = null;
    }
    const chans = [...this.channels.values()];
    this.channels.clear();
    this.reserved.clear();
    for (const c of chans) c._fail(e);
    for (const { ch, reject } of this.pendingOpen.values()) {
      ch._fail(e);
      reject(SessionError.ended(e));
    }
    this.pendingOpen.clear();
    for (const p of this.pending.splice(0)) p.reject(SessionError.ended(e));
    this.resolveClosed(e);
  }

  private resolved = false;

  private fatal(reason: string): void {
    this.trySendCtrl({ type: "error", code: ErrorCode.ProtocolViolation, message: reason });
    this.end = { kind: "protocol", reason };
    try {
      this.ws.close(1002, reason.slice(0, 100));
    } catch {
      /* ignore */
    }
    this.finish(this.end);
  }

  private fatalHandshake(reason: string): void {
    this.end = { kind: "protocol", reason };
    try {
      this.ws.close(1002, reason.slice(0, 100));
    } catch {
      /* ignore */
    }
    this.finish(this.end);
  }

  // ---- sending ----

  /** @internal */
  sendFrame(f: Frame): void {
    if (this.end) return;
    this.ws.send(encodeFrame(f));
  }

  private sendCtrl(m: Ctrl): void {
    this.sendFrame(ctrlFrame(m));
  }

  private trySendCtrl(m: Ctrl): void {
    try {
      this.sendCtrl(m);
    } catch {
      /* socket may be gone */
    }
  }

  /** Send any CTRL message (escape hatch; prefer the typed methods). */
  send(m: Ctrl): void {
    if (this.end) throw SessionError.ended(this.end);
    this.sendCtrl(m);
  }

  /** @internal */
  forget(ch: Channel): void {
    if (this.channels.get(ch.id) === ch) this.channels.delete(ch.id);
  }

  private inUse(id: number): boolean {
    return this.channels.has(id) || this.reserved.has(id);
  }

  private alloc(): number {
    for (let i = 0; i < 1024; i++) {
      this.nextId = this.nextId >= 0xffffffff ? 1 : this.nextId + 1;
      if (!this.inUse(this.nextId)) return this.nextId;
    }
    throw new SessionError("channel_in_use", "no free channel id");
  }

  private alive(): void {
    if (this.end) throw SessionError.ended(this.end);
  }

  // ---- client API ----

  /** Open a pty channel with a fresh id and wait for `opened`. */
  async openPty(opts: { cols: number; rows: number; command?: string[] }): Promise<Channel> {
    this.alive();
    const id = this.alloc();
    const ch = new Channel(id, this);
    this.reserved.add(id);
    const p = new Promise<Channel>((resolve, reject) => this.pendingOpen.set(id, { ch, resolve, reject }));
    const m: Ctrl = { type: "open", channel: id, kind: "pty", cols: opts.cols, rows: opts.rows };
    if (opts.command) m.command = opts.command;
    this.sendCtrl(m);
    return p;
  }

  /** Ask the server to resize a pty channel (no reply). */
  resize(channel: Channel | number, cols: number, rows: number): void {
    this.send({ type: "resize", channel: typeof channel === "number" ? channel : channel.id, cols, rows });
  }

  /** `run_start`. With `log` set, resolves with the transcript channel (DATA_JSON). */
  async runStart(req: RunStartRequest): Promise<RunStarted> {
    this.alive();
    const { log, ...rest } = req;
    let logCh: Channel | undefined;
    let logId: number | null = null;
    if (log !== undefined && log !== false) {
      if (log === true) logId = this.alloc();
      else if (log === 0) throw new SessionError("channel_in_use", "channel 0 is reserved");
      else if (this.inUse(log)) throw new SessionError("channel_in_use", `channel ${log} is already in use`);
      else logId = log;
      this.reserved.add(logId);
      logCh = new Channel(logId, this);
    }
    const p = new Promise<RunStarted>((resolve, reject) =>
      this.pending.push({ kind: "run_start", run_id: rest.run_id, log: logCh, resolve, reject }),
    );
    this.sendCtrl({ type: "run_start", ...rest, log_channel: logId });
    return p;
  }

  /** `run_status`, resolved by the matching `run_state` reply (`unknown_run` rejects with code "remote"). */
  async runStatus(runId: string): Promise<RunInfo> {
    this.alive();
    const p = new Promise<RunInfo>((resolve, reject) =>
      this.pending.push({ kind: "run_status", run_id: runId, resolve, reject }),
    );
    this.sendCtrl({ type: "run_status", run_id: runId });
    return p;
  }

  /** `run_cancel`; the new state arrives as a pushed `run_state` through `onCtrl`. */
  runCancel(runId: string): void {
    this.send({ type: "run_cancel", run_id: runId });
  }

  // ---- inbound ----

  private onMessage(data: unknown): void {
    if (this.end) return;
    let bytes: Uint8Array;
    if (data instanceof ArrayBuffer) bytes = new Uint8Array(data);
    else if (ArrayBuffer.isView(data)) bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    else return this.fatal("non-binary websocket message");
    let f: Frame;
    try {
      f = decodeFrame(bytes);
    } catch (e) {
      return this.fatal(`bad frame: ${e instanceof FrameError ? `${e.code}: ${e.message}` : String(e)}`);
    }
    if (this.handshake) return this.onHandshakeFrame(f);
    this.handleFrame(f);
  }

  private onHandshakeFrame(f: Frame): void {
    const hs = this.handshake!;
    let m: Ctrl | undefined;
    try {
      if (f.type === FrameType.Ctrl) m = parseCtrl(f);
    } catch {
      /* falls through */
    }
    clearTimeout(hs.timer);
    if (m?.type === "welcome" && (this.opts.versions ?? [1]).includes(m.version)) {
      this.version = m.version;
      this.handshake = null;
      hs.resolve();
    } else if (m?.type === "error") {
      this.handshake = null;
      this.end = { kind: "protocol", reason: `${m.code}: ${m.message}` };
      hs.reject(new SessionError("handshake", `server refused handshake: ${m.code}: ${m.message}`, { remote: m }));
      try {
        this.ws.close(1000);
      } catch {
        /* ignore */
      }
      this.finish(this.end);
    } else {
      this.fatalHandshake("expected CTRL welcome");
    }
  }

  private handleFrame(f: Frame): void {
    switch (f.type) {
      case FrameType.Ctrl: {
        let m: Ctrl;
        try {
          m = parseCtrl(f);
        } catch (e) {
          const unknown = e instanceof UnknownCtrlType;
          this.trySendCtrl({
            type: "error",
            code: unknown ? ErrorCode.UnknownMessage : ErrorCode.ProtocolViolation,
            message: String((e as Error).message),
          });
          return;
        }
        this.handleCtrl(m);
        return;
      }
      case FrameType.DataBin:
      case FrameType.DataJson: {
        const ch = this.channels.get(f.channel);
        if (!ch) {
          this.trySendCtrl({
            type: "error",
            code: ErrorCode.UnknownChannel,
            message: `data for unknown channel ${f.channel}`,
          });
          return;
        }
        try {
          ch._onData(f.type === FrameType.DataJson, f.payload);
        } catch (e) {
          this.violation(ch, e);
        }
        return;
      }
      case FrameType.Window: {
        const ch = this.channels.get(f.channel);
        if (!ch) return; // ignored for unknown/closed channels
        try {
          ch._onWindow(windowCredit(f));
        } catch (e) {
          this.violation(ch, e);
        }
        return;
      }
      case FrameType.Close: {
        this.channels.get(f.channel)?._onClose(closeInfo(f));
        return;
      }
    }
  }

  private violation(ch: Channel, e: unknown): void {
    const msg = (e as Error).message;
    this.trySendCtrl({ type: "error", code: ErrorCode.FlowControl, message: `channel ${ch.id}: ${msg}` });
    ch._abort(msg);
  }

  private emit(m: Ctrl): void {
    for (const l of [...this.listeners]) {
      try {
        l(m);
      } catch {
        /* listener errors must not break the session */
      }
    }
  }

  private handleCtrl(m: Ctrl): void {
    switch (m.type) {
      case "opened": {
        const p = this.pendingOpen.get(m.channel);
        if (!p) {
          this.trySendCtrl({
            type: "error",
            code: ErrorCode.ProtocolViolation,
            message: `unsolicited opened for channel ${m.channel}`,
          });
          return;
        }
        this.pendingOpen.delete(m.channel);
        this.reserved.delete(m.channel);
        this.channels.set(m.channel, p.ch);
        p.resolve(p.ch);
        return;
      }
      case "open_error": {
        const p = this.pendingOpen.get(m.channel);
        this.reserved.delete(m.channel);
        if (p) {
          this.pendingOpen.delete(m.channel);
          p.reject(new SessionError("open_rejected", `open rejected: ${m.message}`, { remote: { code: "open_error", message: m.message } }));
        }
        return;
      }
      case "run_accepted": {
        const i = this.pending.findIndex((p) => p.kind === "run_start" && p.run_id === m.run_id);
        const p = i >= 0 ? this.pending.splice(i, 1)[0] : undefined;
        if (!p || p.kind !== "run_start") {
          this.trySendCtrl({
            type: "error",
            code: ErrorCode.ProtocolViolation,
            message: `unsolicited run_accepted for ${m.run_id}`,
          });
          return;
        }
        if (p.log) {
          this.reserved.delete(p.log.id);
          this.channels.set(p.log.id, p.log);
        }
        p.resolve({ accepted: { run_id: m.run_id, state: m.state, existing: m.existing }, log: p.log });
        return;
      }
      case "run_state": {
        const i = this.pending.findIndex((p) => p.kind === "run_status" && p.run_id === m.run_id);
        const p = i >= 0 ? this.pending.splice(i, 1)[0] : undefined;
        if (p && p.kind === "run_status") {
          p.resolve({ run_id: m.run_id, state: m.state, exit_code: m.exit_code, summary: m.summary });
        } else {
          this.emit(m);
        }
        return;
      }
      case "error": {
        // An explicit run_id wins (any code); else run-related codes hit the oldest pending.
        let idx = -1;
        if (typeof m.run_id === "string") idx = this.pending.findIndex((q) => q.run_id === m.run_id);
        else if (ATTRIBUTABLE.has(m.code)) idx = this.pending.length > 0 ? 0 : -1;
        if (idx >= 0) {
          const p = this.pending.splice(idx, 1)[0]!;
          if (p.kind === "run_start" && p.log) this.reserved.delete(p.log.id);
          p.reject(new SessionError("remote", `peer error ${m.code}: ${m.message}`, { remote: m }));
          return;
        }
        this.emit(m);
        return;
      }
      case "credential_captured":
        this.emit(m);
        return;
      default:
        this.trySendCtrl({
          type: "error",
          code: ErrorCode.ProtocolViolation,
          message: `unexpected message for the client side: ${m.type}`,
        });
    }
  }
}
