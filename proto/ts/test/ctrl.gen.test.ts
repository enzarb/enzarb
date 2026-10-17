import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { CTRL_TYPES, SECRET_PATHS, Secret, asCtrl } from "../src/index.js";

const vectors = JSON.parse(readFileSync(new URL("../../vectors/ctrl.json", import.meta.url), "utf8")) as {
  name: string;
  message: Record<string, unknown>;
}[];

describe("ctrl.gen.ts", () => {
  it("CTRL_TYPES is exactly the set of vector tags", () => {
    const tags = new Set(vectors.map((v) => v.message.type as string));
    expect(new Set<string>(CTRL_TYPES)).toEqual(tags);
    expect(CTRL_TYPES.length).toBe(tags.size);
  });

  it("SECRET_PATHS covers the known secret fields", () => {
    expect(SECRET_PATHS).toEqual({
      open: [["claude_token"]],
      credential_captured: [["token"]],
      step_start: [["env", "*"], ["files", "*"], ["github_token"], ["workspaces", "*", "token"]],
    });
  });

  it("every secret in the vectors is wrapped, nothing else is", () => {
    let wrapped = 0;
    const walk = (v: unknown) => {
      if (v instanceof Secret) wrapped++;
      else if (typeof v === "object" && v !== null) Object.values(v).forEach(walk);
    };
    let expected = 0;
    for (const { message } of vectors) {
      walk(asCtrl(message));
      // Count the string leaves at secret paths in the raw message.
      const count = (v: unknown, p: readonly string[]): number => {
        if (p.length === 0) return typeof v === "string" ? 1 : 0;
        if (typeof v !== "object" || v === null) return 0;
        const [h, ...rest] = p;
        if (h === "*") return Object.values(v).reduce<number>((n, x) => n + count(x, rest), 0);
        return count((v as Record<string, unknown>)[h!], rest);
      };
      for (const p of SECRET_PATHS[message.type as string] ?? []) expected += count(message, p);
    }
    expect(expected).toBeGreaterThan(0);
    expect(wrapped).toBe(expected);
  });
});
