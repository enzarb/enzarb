// A wrapper that keeps tokens out of logs. The wire format is a plain string.

const REDACTED = "[redacted]";

/**
 * A secret string (token). `String()`, template strings, `JSON.stringify` and `util.inspect` /
 * `console.log` all yield `[redacted]`; read the value with `expose()`. CTRL encoding unwraps it
 * explicitly (see `secretReplacer`), so the wire carries a plain string.
 */
export class Secret {
  readonly #value: string;

  constructor(value: string) {
    this.#value = value;
  }

  /** The secret value. Do not log it. */
  expose(): string {
    return this.#value;
  }

  toString(): string {
    return REDACTED;
  }

  toJSON(): string {
    return REDACTED;
  }

  [Symbol.for("nodejs.util.inspect.custom")](): string {
    return `Secret(${REDACTED})`;
  }
}

/** Accepted wherever a token is sent; plain strings are wrapped. */
export type SecretLike = Secret | string;

export function toSecret(v: SecretLike): Secret {
  return v instanceof Secret ? v : new Secret(v);
}

/**
 * `JSON.stringify` replacer that writes `Secret`s as their plain value. Only for encoding wire
 * frames. `toJSON` runs before the replacer, so the original is read from `this[key]`.
 */
export function secretReplacer(this: unknown, key: string, value: unknown): unknown {
  const orig = (this as Record<string, unknown>)[key];
  return orig instanceof Secret ? orig.expose() : value;
}
