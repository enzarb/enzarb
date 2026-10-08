import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  FrameError,
  asCtrl,
  ctrlFrame,
  decodeFrame,
  encodeFrame,
  frameTypeName,
  parseCtrl,
  parseJson,
  type Frame,
  type FrameType,
} from "../src/index.js";

const dir = new URL("../../vectors/", import.meta.url);
const load = (name: string) => JSON.parse(readFileSync(new URL(name, dir), "utf8")) as any[];

const fromHex = (h: string) => Uint8Array.from(Buffer.from(h, "hex"));
const toHex = (b: Uint8Array) => Buffer.from(b).toString("hex");
const TYPES: Record<string, FrameType> = { CTRL: 1, DATA_BIN: 2, DATA_JSON: 3, WINDOW: 4, CLOSE: 5 };

describe("frames.json", () => {
  const vectors = load("frames.json");
  it("has vectors", () => expect(vectors.length).toBeGreaterThan(0));
  for (const v of vectors) {
    it(`${v.name}: decode`, () => {
      const f = decodeFrame(fromHex(v.hex));
      expect(frameTypeName(f.type)).toBe(v.frame.type);
      expect(f.flags).toBe(v.frame.flags);
      expect(f.channel).toBe(v.frame.channel);
      expect(toHex(f.payload)).toBe(v.frame.payload_hex);
    });
    it(`${v.name}: encode`, () => {
      const f: Frame = {
        type: TYPES[v.frame.type]!,
        flags: v.frame.flags,
        channel: v.frame.channel,
        payload: fromHex(v.frame.payload_hex),
      };
      expect(toHex(encodeFrame(f))).toBe(v.hex);
    });
  }
});

describe("ctrl.json", () => {
  const vectors = load("ctrl.json");
  it("has vectors", () => expect(vectors.length).toBeGreaterThan(0));
  for (const v of vectors) {
    it(`${v.name}: decode`, () => {
      const f = decodeFrame(fromHex(v.hex));
      expect(f.channel).toBe(0);
      expect(parseCtrl(f)).toEqual(v.message);
    });
    // CTRL `hex` is Rust-canonical (struct field order), so compare parsed JSON, not bytes.
    it(`${v.name}: encode (parsed JSON equality)`, () => {
      const enc = encodeFrame(ctrlFrame(asCtrl(v.message)));
      const dec = decodeFrame(enc);
      const expected = parseJson(decodeFrame(fromHex(v.hex)).payload);
      expect(parseJson(dec.payload)).toEqual(expected);
    });
  }
});

describe("json_types.json", () => {
  for (const v of load("json_types.json")) {
    it(`${v.name}: round trips`, () => {
      expect(JSON.parse(JSON.stringify(v.json))).toEqual(v.json);
      expect(v.json.run_id).toBeTypeOf("string");
    });
  }
});

describe("errors.json", () => {
  for (const v of load("errors.json")) {
    it(`${v.name}: fails with ${v.error}`, () => {
      try {
        decodeFrame(fromHex(v.hex));
      } catch (e) {
        expect(e).toBeInstanceOf(FrameError);
        expect((e as FrameError).code).toBe(v.error);
        return;
      }
      throw new Error("expected decode to fail");
    });
  }
});
