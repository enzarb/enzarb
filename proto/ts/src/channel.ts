import { SessionError, type SessionEnd } from "./errors.js";
import { FlowControl } from "./flow.js";
import {
  CloseCode,
  type CloseInfo,
  type Frame,
  INITIAL_WINDOW,
  closeFrame,
  dataBin,
  dataJson,
  parseJson,
  windowFrame,
} from "./frame.js";

/** Largest DATA_BIN frame the runtime emits (the spec recommends <= 16 KiB). */
export const SEND_CHUNK = 16 * 1024;

const enc = new TextEncoder();

/** One received data frame. */
export interface ChannelData {
  kind: "bin" | "json";
  data: Uint8Array;
}

/** What a channel needs from its session. */
export interface ChannelHost {
  sendFrame(f: Frame): void;
  forget(ch: Channel): void;
}

type Fail = { type: "session"; end: SessionEnd } | { type: "aborted"; reason: string };

/**
 * A data channel. Writes wait for the peer's credit (backpressure, nothing is
 * dropped); credit is returned to the peer automatically as you `read()`.
 * Also async-iterable: `for await (const d of channel) {...}`.
 */
export class Channel implements AsyncIterable<ChannelData> {
  private flow = new FlowControl();
  private inbox: ChannelData[] = [];
  private readers: Array<() => void> = [];
  private creditWaiter: (() => void) | null = null;
  private sendQueue: Promise<void> = Promise.resolve();
  private failed: Fail | null = null;
  private remoteClose: CloseInfo | null = null;

  /** @internal */
  constructor(
    readonly id: number,
    private host: ChannelHost,
  ) {}

  /** Credit the peer currently allows us to send. */
  get credit(): number {
    return this.flow.sendCredit;
  }

  /** The peer's CLOSE code and reason, once received. */
  get closeInfo(): CloseInfo | null {
    return this.remoteClose;
  }

  get isLocalClosed(): boolean {
    return this.flow.isLocalClosed;
  }

  // ---- sending ----

  private enqueue(fn: () => Promise<void>): Promise<void> {
    const p = this.sendQueue.then(fn);
    this.sendQueue = p.catch(() => undefined);
    return p;
  }

  private failError(): SessionError | null {
    const f = this.failed;
    if (!f) return null;
    return f.type === "session"
      ? SessionError.ended(f.end)
      : new SessionError("channel_aborted", `channel ${this.id} aborted: ${f.reason}`);
  }

  private async reserve(want: number, exact: boolean): Promise<number> {
    for (;;) {
      const err = this.failError();
      if (err) throw err;
      if (this.flow.isLocalClosed) throw new SessionError("channel_closed", `channel ${this.id} is closed for sending`);
      const n = this.flow.sendable(want);
      if (n > 0 && (!exact || n >= want)) {
        this.flow.onSend(n);
        return n;
      }
      await new Promise<void>((r) => {
        this.creditWaiter = r;
      });
    }
  }

  /** Send bytes (or a UTF-8 string) as DATA_BIN frames, waiting for credit as needed. */
  write(data: Uint8Array | string): Promise<void> {
    const bytes = typeof data === "string" ? enc.encode(data) : data;
    return this.enqueue(async () => {
      let off = 0;
      while (off < bytes.length) {
        const n = await this.reserve(Math.min(bytes.length - off, SEND_CHUNK), false);
        this.host.sendFrame(dataBin(this.id, bytes.subarray(off, off + n)));
        off += n;
      }
    });
  }

  /** Send one DATA_JSON frame (at most 256 KiB serialized). */
  writeJson(value: unknown): Promise<void> {
    const body = JSON.stringify(value);
    const len = enc.encode(body).length;
    if (len > INITIAL_WINDOW) {
      return Promise.reject(new SessionError("json_too_large", `json payload of ${len} bytes exceeds the 256 KiB window`));
    }
    return this.enqueue(async () => {
      await this.reserve(len, true);
      this.host.sendFrame(dataJson(this.id, value));
    });
  }

  /**
   * Half-close: send CLOSE after queued writes. Receiving continues until the
   * peer closes. For a pty channel this also terminates the process.
   */
  close(code: number = CloseCode.Normal, reason = ""): Promise<void> {
    return this.enqueue(async () => {
      if (this.failed || this.flow.isLocalClosed) return;
      this.closeNow(code, reason);
    });
  }

  private closeNow(code: number, reason: string): void {
    this.flow.closeLocal();
    this.host.sendFrame(closeFrame(this.id, code, reason));
    this.wakeCredit();
    if (this.flow.isClosed) this.host.forget(this);
  }

  // ---- receiving ----

  /** Next data frame, or `null` once the peer closed (or the session ended) and all was delivered. */
  async read(): Promise<ChannelData | null> {
    for (;;) {
      const d = this.inbox.shift();
      if (d) {
        const g = this.flow.release(d.data.length);
        if (g !== null) this.host.sendFrame(windowFrame(this.id, g));
        return d;
      }
      if (this.remoteClose) return null;
      const err = this.failError();
      if (err) throw err;
      await new Promise<void>((r) => this.readers.push(r));
    }
  }

  /** Next frame parsed as JSON, or `null` at end. */
  async readJson<T = unknown>(): Promise<T | null> {
    const d = await this.read();
    return d === null ? null : (parseJson(d.data) as T);
  }

  async *[Symbol.asyncIterator](): AsyncIterator<ChannelData> {
    for (;;) {
      const d = await this.read();
      if (d === null) return;
      yield d;
    }
  }

  // ---- called by the session ----

  private wakeReaders(): void {
    const rs = this.readers;
    this.readers = [];
    for (const r of rs) r();
  }

  private wakeCredit(): void {
    const w = this.creditWaiter;
    this.creditWaiter = null;
    w?.();
  }

  /** @internal Throws FlowError on a violation. */
  _onData(json: boolean, payload: Uint8Array): void {
    this.flow.onData(payload.length);
    if (this.failed) {
      const g = this.flow.release(payload.length) ?? this.flow.flushGrant();
      if (g !== null) this.host.sendFrame(windowFrame(this.id, g));
      return;
    }
    this.inbox.push({ kind: json ? "json" : "bin", data: payload });
    this.wakeReaders();
  }

  /** @internal Throws FlowError on a violation. */
  _onWindow(credit: number): void {
    this.flow.onWindow(credit);
    this.wakeCredit();
  }

  /** @internal */
  _onClose(info: CloseInfo): void {
    this.flow.onCloseRemote();
    this.remoteClose = info;
    this.wakeReaders();
    this.wakeCredit();
    if (this.flow.isClosed) this.host.forget(this);
  }

  /** @internal Protocol violation on this channel: CLOSE code 1 and fail it. */
  _abort(reason: string): void {
    if (!this.flow.isLocalClosed) {
      this.flow.closeLocal();
      this.host.sendFrame(closeFrame(this.id, CloseCode.ProtocolError, reason));
    }
    this.failed = { type: "aborted", reason };
    this.inbox = [];
    this.wakeReaders();
    this.wakeCredit();
    this.host.forget(this);
  }

  /** @internal The session ended. */
  _fail(end: SessionEnd): void {
    this.failed ??= { type: "session", end };
    this.wakeReaders();
    this.wakeCredit();
  }
}
