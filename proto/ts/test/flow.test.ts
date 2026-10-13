import { describe, expect, it } from "vitest";
import { FlowControl, FlowError, FrameDecoder, INITIAL_WINDOW, MAX_CREDIT, closeFrame, dataBin, encodeFrame, negotiateVersion, windowFrame } from "../src/index.js";

describe("FlowControl", () => {
  it("sender cannot exceed credit", () => {
    const f = new FlowControl();
    f.onSend(INITIAL_WINDOW);
    expect(f.sendable(10)).toBe(0);
    expect(() => f.onSend(1)).toThrow(FlowError);
  });

  it("window adds credit and is bounded", () => {
    const f = new FlowControl();
    f.onSend(INITIAL_WINDOW);
    f.onWindow(100);
    expect(f.sendCredit).toBe(100);
    expect(() => f.onWindow(MAX_CREDIT)).toThrowError(/2\^31/);
  });

  it("release batches until half the window is consumed", () => {
    const f = new FlowControl();
    f.onData(INITIAL_WINDOW);
    expect(() => f.onData(1)).toThrow(FlowError);
    expect(f.release(INITIAL_WINDOW / 2 - 1)).toBeNull();
    expect(f.release(1)).toBe(INITIAL_WINDOW / 2);
    expect(f.recvGranted).toBe(INITIAL_WINDOW / 2);
    f.onData(10);
    expect(f.release(10)).toBeNull();
    expect(f.flushGrant()).toBe(10);
    expect(() => f.release(INITIAL_WINDOW)).toThrow(FlowError);
  });

  it("close semantics", () => {
    const f = new FlowControl();
    f.closeLocal();
    expect(f.sendable(5)).toBe(0);
    expect(() => f.onSend(1)).toThrow(FlowError);
    expect(() => f.closeLocal()).toThrow(FlowError);
    f.onWindow(5); // ignored
    expect(f.sendCredit).toBe(0);
    f.onCloseRemote();
    expect(() => f.onData(1)).toThrow(FlowError);
    expect(f.isClosed).toBe(true);
  });
});

describe("FrameDecoder", () => {
  it("handles arbitrary chunking", () => {
    const frames = [dataBin(1, new Uint8Array([1, 2, 3])), windowFrame(1, 5), closeFrame(1, 2, "bye")];
    const all = frames.map(encodeFrame);
    const bytes = new Uint8Array(all.reduce((n, b) => n + b.length, 0));
    let o = 0;
    for (const b of all) {
      bytes.set(b, o);
      o += b.length;
    }
    for (const step of [1, 3, 7, bytes.length]) {
      const d = new FrameDecoder();
      const got = [];
      for (let i = 0; i < bytes.length; i += step) {
        d.push(bytes.subarray(i, i + step));
        for (let f = d.next(); f; f = d.next()) got.push(f);
      }
      expect(got.map((f) => f.type)).toEqual(frames.map((f) => f.type));
    }
  });

  it("rejects a bad header as soon as it is available", () => {
    const d = new FrameDecoder();
    d.push(new Uint8Array([9, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
    expect(() => d.next()).toThrow(/unknown frame type/);
  });
});

it("negotiates the highest common version", () => {
  expect(negotiateVersion([1, 2, 3], [1, 2])).toBe(2);
  expect(negotiateVersion([5], [1])).toBeNull();
});
