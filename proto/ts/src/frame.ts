// Frame layout and codec. Mirrors crates/enzarb-proto/src/frame.rs; see docs/protocol.md.

export const HEADER_LEN = 10;
export const MAX_PAYLOAD_LEN = 1 << 20;
export const MAX_CREDIT = 2 ** 31 - 1;
export const INITIAL_WINDOW = 256 * 1024;

export const CloseCode = {
  Normal: 0,
  ProtocolError: 1,
  GoingAway: 2,
  Cancelled: 3,
} as const;

export const FrameType = {
  Ctrl: 1,
  DataBin: 2,
  DataJson: 3,
  Window: 4,
  Close: 5,
} as const;
export type FrameType = (typeof FrameType)[keyof typeof FrameType];

const NAMES: Record<number, string> = {
  1: "CTRL",
  2: "DATA_BIN",
  3: "DATA_JSON",
  4: "WINDOW",
  5: "CLOSE",
};

/** Name used in the spec and the shared vectors. */
export function frameTypeName(t: number): string {
  return NAMES[t] ?? `UNKNOWN(${t})`;
}

export type FrameErrorCode =
  | "truncated"
  | "trailing_bytes"
  | "unknown_type"
  | "unknown_flags"
  | "payload_too_large"
  | "ctrl_channel"
  | "zero_channel"
  | "empty_data"
  | "bad_window_payload"
  | "bad_credit"
  | "bad_close_payload"
  | "json";

/** Codec error; `code` is the stable identifier shared with `vectors/errors.json`. */
export class FrameError extends Error {
  constructor(
    readonly code: FrameErrorCode,
    message?: string,
  ) {
    super(message ?? code);
    this.name = "FrameError";
  }
}

export interface Frame {
  type: FrameType;
  flags: number;
  channel: number;
  payload: Uint8Array;
}

export interface FrameHeader {
  type: FrameType;
  flags: number;
  channel: number;
  /** Payload length (already checked against MAX_PAYLOAD_LEN). */
  len: number;
}

const encoder = new TextEncoder();
const strictDecoder = new TextDecoder("utf-8", { fatal: true });

function checkChannel(type: number, channel: number): void {
  if (type === FrameType.Ctrl) {
    if (channel !== 0) throw new FrameError("ctrl_channel");
  } else if (channel === 0) {
    throw new FrameError("zero_channel", `${frameTypeName(type)} frames must not use channel 0`);
  }
}

/** Parse and validate the 10-byte header at the start of `b` (which may be longer). */
export function parseHeader(b: Uint8Array): FrameHeader {
  if (b.length < HEADER_LEN) throw new FrameError("truncated");
  const type = b[0]!;
  if (!(type in NAMES)) throw new FrameError("unknown_type", `unknown frame type ${type}`);
  const flags = b[1]!;
  if (flags !== 0) throw new FrameError("unknown_flags");
  const dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
  const channel = dv.getUint32(2);
  const len = dv.getUint32(6);
  if (len > MAX_PAYLOAD_LEN) throw new FrameError("payload_too_large");
  checkChannel(type, channel);
  return { type: type as FrameType, flags, channel, len };
}

/** Structural checks shared by encode and decode. */
export function validateFrame(f: Frame): void {
  if (f.flags !== 0) throw new FrameError("unknown_flags");
  if (f.payload.length > MAX_PAYLOAD_LEN) throw new FrameError("payload_too_large");
  checkChannel(f.type, f.channel);
  switch (f.type) {
    case FrameType.DataBin:
    case FrameType.DataJson:
      if (f.payload.length === 0) throw new FrameError("empty_data");
      break;
    case FrameType.Window:
      windowCredit(f);
      break;
    case FrameType.Close:
      closeInfo(f);
      break;
  }
}

export function encodeFrame(f: Frame): Uint8Array<ArrayBuffer> {
  validateFrame(f);
  const out = new Uint8Array(HEADER_LEN + f.payload.length);
  const dv = new DataView(out.buffer);
  out[0] = f.type;
  out[1] = f.flags;
  dv.setUint32(2, f.channel);
  dv.setUint32(6, f.payload.length);
  out.set(f.payload, HEADER_LEN);
  return out;
}

/** Decode exactly one frame from a complete websocket message. */
export function decodeFrame(msg: Uint8Array): Frame {
  const h = parseHeader(msg);
  const total = HEADER_LEN + h.len;
  if (msg.length < total) throw new FrameError("truncated");
  if (msg.length > total) throw new FrameError("trailing_bytes", `${msg.length - total} trailing bytes`);
  const f: Frame = { type: h.type, flags: h.flags, channel: h.channel, payload: msg.subarray(HEADER_LEN) };
  validateFrame(f);
  return f;
}

/** Incremental decoder for byte-stream transports. After a throw the stream is unrecoverable. */
export class FrameDecoder {
  private buf = new Uint8Array(0);

  push(chunk: Uint8Array): void {
    const n = new Uint8Array(this.buf.length + chunk.length);
    n.set(this.buf);
    n.set(chunk, this.buf.length);
    this.buf = n;
  }

  /** Next complete frame, or `null` if more bytes are needed. */
  next(): Frame | null {
    if (this.buf.length < HEADER_LEN) return null;
    const h = parseHeader(this.buf);
    const total = HEADER_LEN + h.len;
    if (this.buf.length < total) return null;
    const payload = this.buf.slice(HEADER_LEN, total);
    this.buf = this.buf.slice(total);
    const f: Frame = { type: h.type, flags: h.flags, channel: h.channel, payload };
    validateFrame(f);
    return f;
  }
}

// ---- constructors ----

export function frame(type: FrameType, channel: number, payload: Uint8Array = new Uint8Array(0)): Frame {
  return { type, flags: 0, channel, payload };
}

/** `CTRL` frame carrying `msg` as compact JSON. */
export function ctrlFrame(msg: object): Frame {
  return frame(FrameType.Ctrl, 0, encoder.encode(JSON.stringify(msg)));
}

export function dataBin(channel: number, payload: Uint8Array): Frame {
  return frame(FrameType.DataBin, channel, payload);
}

export function dataJson(channel: number, value: unknown): Frame {
  return frame(FrameType.DataJson, channel, encoder.encode(JSON.stringify(value)));
}

export function windowFrame(channel: number, credit: number): Frame {
  const p = new Uint8Array(4);
  new DataView(p.buffer).setUint32(0, credit);
  return frame(FrameType.Window, channel, p);
}

/** `CLOSE` frame. Code 0 with no reason encodes as an empty payload. */
export function closeFrame(channel: number, code: number = CloseCode.Normal, reason = ""): Frame {
  if (code === 0 && reason === "") return frame(FrameType.Close, channel);
  const r = encoder.encode(reason);
  const p = new Uint8Array(2 + r.length);
  new DataView(p.buffer).setUint16(0, code);
  p.set(r, 2);
  return frame(FrameType.Close, channel, p);
}

// ---- accessors ----

/** Parse the payload of a `CTRL` frame (untyped; see `parseCtrl` in ctrl.ts). */
export function ctrlJson(f: Frame): unknown {
  if (f.type !== FrameType.Ctrl) throw new FrameError("ctrl_channel");
  return parseJson(f.payload);
}

export function parseJson(payload: Uint8Array): unknown {
  try {
    return JSON.parse(strictDecoder.decode(payload));
  } catch (e) {
    throw new FrameError("json", String(e));
  }
}

export function windowCredit(f: Frame): number {
  if (f.payload.length !== 4) throw new FrameError("bad_window_payload");
  const c = new DataView(f.payload.buffer, f.payload.byteOffset, 4).getUint32(0);
  if (c === 0 || c > MAX_CREDIT) throw new FrameError("bad_credit");
  return c;
}

export interface CloseInfo {
  code: number;
  reason: string;
}

export function closeInfo(f: Frame): CloseInfo {
  const n = f.payload.length;
  if (n === 0) return { code: CloseCode.Normal, reason: "" };
  if (n === 1) throw new FrameError("bad_close_payload");
  const code = new DataView(f.payload.buffer, f.payload.byteOffset, 2).getUint16(0);
  try {
    return { code, reason: strictDecoder.decode(f.payload.subarray(2)) };
  } catch {
    throw new FrameError("bad_close_payload");
  }
}
