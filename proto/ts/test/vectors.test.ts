import { readFileSync } from "node:fs";
import { inspect } from "node:util";
import { describe, expect, it } from "vitest";
import { secretReplacer } from "../src/secret.js";
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
      // Tokens decode as `Secret`; compare their wire (exposed) form.
      expect(JSON.parse(JSON.stringify(parseCtrl(f), secretReplacer))).toEqual(v.message);
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

describe("graph segment vectors", () => {
  it("run_start_graph and run_finished_graph carry the graph fields", () => {
    const rs = load("ctrl.json").find((v) => v.name === "run_start_graph")!;
    expect(rs.message.graph).toMatchObject({ start: "implement", prompts: { implement: "Implement issue #42" } });
    const fin = load("json_types.json").find((v) => v.name === "run_finished_graph")!;
    expect(fin.json.graph).toMatchObject({ stopped_at: "open_pr", visits: { implement: 1 } });
  });
});

describe("per-user claude token vectors", () => {
  const vectors = load("ctrl.json");
  const byName = (n: string) => vectors.find((v) => v.name === n)!;
  const NEW_FIELDS = ["claude_token", "token", "legacy", "scope"];

  it("token vectors exist and carry the new fields", () => {
    expect(byName("run_start_claude_token").message.claude_token).toBeTypeOf("string");
    expect(byName("open_claude_token").message.claude_token).toBeTypeOf("string");
    expect(byName("credential_captured_token").message).toMatchObject({ legacy: true });
    expect(byName("credential_state_scope").message.scope).toBe("user");
    expect(byName("credential_clear").message).toEqual({ type: "credential_clear", kind: "claude_oauth" });
  });

  it("old decoders (new fields stripped) still read the shared fields", () => {
    for (const v of vectors.filter((v) => NEW_FIELDS.some((k) => k in v.message))) {
      const old = Object.fromEntries(Object.entries(v.message).filter(([k]) => !NEW_FIELDS.includes(k)));
      expect(asCtrl(old)).toMatchObject(old);
    }
  });

  it("unknown keys are tolerated", () => {
    const m = asCtrl({ type: "credential_state", kind: "claude_oauth", configured: true, scope: "org", extra: 1 });
    expect(m).toMatchObject({ scope: "org" });
  });

  it("a decoded token never appears in inspect/String/JSON output", () => {
    const f = decodeFrame(fromHex(byName("open_claude_token").hex));
    const m = parseCtrl(f);
    const token = byName("open_claude_token").message.claude_token as string;
    for (const text of [inspect(m, { depth: 5 }), JSON.stringify(m), String((m as { claude_token?: unknown }).claude_token)]) {
      expect(text).not.toContain(token);
    }
  });
});

describe("ctrl_compat.json", () => {
  const vectors = load("ctrl_compat.json");
  it("has vectors", () => expect(vectors.length).toBeGreaterThan(0));
  for (const v of vectors) {
    it(`${v.name}: decodes and the shared fields agree`, () => {
      const m = asCtrl(JSON.parse(v.payload));
      expect(m).toMatchObject(v.expect);
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
