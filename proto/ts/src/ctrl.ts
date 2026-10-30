// CTRL messages (contracts section 4). Field names are the wire names, mirroring
// crates/enzarb-proto/src/ctrl.rs.

import { type Frame, FrameError, ctrlJson } from "./frame.js";
import { type Ctrl, type EnvironmentKind, SECRET_PATHS, CTRL_TYPES } from "./ctrl.gen.js";
import { Secret } from "./secret.js";

export * from "./ctrl.gen.js";
export { Secret, type SecretLike } from "./secret.js";

export type CtrlType = Ctrl["type"];

const TYPES: ReadonlySet<string> = new Set<string>(CTRL_TYPES);

/** Known `credential_state.scope` values. Unknown values must be tolerated, so `scope` stays an open string. */
export const CredentialScope = { User: "user" } as const;

/**
 * Channel kinds. `acp` carries ACP's newline-delimited JSON-RPC as a byte stream. Only send it after
 * `welcome.features` contained `"acp"` (an old proxy closes the socket on an unknown kind). Received
 * kinds may be any string, so the type stays open.
 */
export type ChannelKind = "pty" | "acp" | (string & {});

const ENVIRONMENT_KINDS: ReadonlySet<string> = new Set(["disk_full", "oom", "toolchain_missing", "network", "load"]);

/**
 * Normalise a received `step_finished.environment.kind`. The decoder passes the wire string through, so a
 * kind added later arrives as-is; anything outside the known set is `"unknown"` (treat it as a generic
 * environment failure).
 */
export function environmentKind(k: unknown): EnvironmentKind {
  return typeof k === "string" && ENVIRONMENT_KINDS.has(k) ? (k as EnvironmentKind) : "unknown";
}

/** Feature names advertised in `welcome.features`. */
/** `ClaudeUserToken` is a client feature (`hello.features`), sent only by sandbox-proxy. */
/** `Stats`: the server answers `stats_request` with `stats`; send the request only after seeing it. */
/** `FileRead`: the server answers `file_read` with `file`; send the request only after seeing it. */
/** `Steps`: the server runs ADR-0022 steps (`step_*`, `session_event`); send `step_*` only after seeing it. */
export const Feature = { Acp: "acp", ClaudeUserToken: "claude_user_token", FileRead: "file_read", Stats: "stats", Steps: "steps" } as const;
/** Decoders keep at most this many features and drop any longer than `MAX_FEATURE_LEN` bytes. */
export const MAX_FEATURES = 64;
export const MAX_FEATURE_LEN = 64;
/** Limits for `open.repo` / `open.resume` (bytes); longer values are refused with `invalid_request`. */
export const MAX_REPO_LEN = 2048;
export const MAX_RESUME_LEN = 256;

/** Standard `open_error.code` values. Unknown codes must be tolerated. */
export const OpenErrorCode = {
  ChatDisabled: "chat_disabled",
  ChatLimit: "chat_limit",
  NoCredential: "no_credential",
  RepoNotCloned: "repo_not_cloned",
  UnsupportedKind: "unsupported_kind",
  InvalidRequest: "invalid_request",
} as const;

/** Typical CLOSE reasons on an `acp` channel (besides `exit:<n>` / `signal:<n>`); tolerate any text. */
export const CloseReason = {
  Idle: "idle",
  Superseded: "superseded",
  Shutdown: "shutdown",
  Limit: "limit",
} as const;

/** Drop non-strings, empty and oversized entries, and cap the count of a peer-supplied feature list. */
export function sanitizeFeatures(v: unknown): string[] {
  if (!Array.isArray(v)) return [];
  const out: string[] = [];
  for (const f of v) {
    if (typeof f !== "string" || f.length === 0 || new TextEncoder().encode(f).length > MAX_FEATURE_LEN) continue;
    out.push(f);
    if (out.length >= MAX_FEATURES) break;
  }
  return out;
}
/** `step_finished.exit` values (an open string on the wire). */
export const StepExit = { Success: "success", Failure: "failure", Interrupted: "interrupted" } as const;
/** Session event kinds (an open set; receivers tolerate others). */
export const SessionKind = {
  Prompt: "prompt",
  User: "user",
  Assistant: "assistant",
  Thinking: "thinking",
  ToolCall: "tool_call",
  ToolResult: "tool_result",
  Usage: "usage",
  Steer: "steer",
  System: "system",
  Stdout: "stdout",
  Stderr: "stderr",
} as const;

export const ErrorCode = {
  UnsupportedVersion: "unsupported_version",
  UnknownMessage: "unknown_message",
  ProtocolViolation: "protocol_violation",
  FlowControl: "flow_control",
  Unauthorized: "unauthorized",
  UnknownChannel: "unknown_channel",
  UnknownRun: "unknown_run",
  Internal: "internal",
  /** The peer does not implement the request (e.g. `step_*` without a step executor). */
  Unimplemented: "unimplemented",
} as const;

/** Thrown for a CTRL payload with an unknown `type` (answer with `error` unknown_message). */
export class UnknownCtrlType extends Error {
  constructor(readonly ctrlType: string) {
    super(`unknown ctrl type ${ctrlType}`);
    this.name = "UnknownCtrlType";
  }
}

/** Validate the tag of an already-parsed CTRL object. Fields are not deeply checked. */
export function asCtrl(v: unknown): Ctrl {
  if (typeof v !== "object" || v === null || Array.isArray(v)) {
    throw new FrameError("json", "CTRL payload is not an object");
  }
  const t = (v as { type?: unknown }).type;
  if (typeof t !== "string") throw new FrameError("json", "CTRL payload has no type");
  if (!TYPES.has(t)) throw new UnknownCtrlType(t);
  // Wrap incoming tokens so logging the message cannot leak them. Unknown keys pass through untouched.
  let out: unknown = v;
  for (const path of SECRET_PATHS[t] ?? []) out = wrapSecrets(out, path);
  return out as Ctrl;
}

/** Copy of `v` with the string at `path` wrapped in a `Secret` (`*` = every map value or array item). */
function wrapSecrets(v: unknown, path: readonly string[]): unknown {
  const [head, ...rest] = path;
  if (head === undefined) return typeof v === "string" ? new Secret(v) : v;
  if (typeof v !== "object" || v === null) return v;
  if (head === "*") {
    if (Array.isArray(v)) return v.map((x) => wrapSecrets(x, rest));
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, wrapSecrets(x, rest)]));
  }
  if (Array.isArray(v) || !(head in v)) return v;
  return { ...v, [head]: wrapSecrets((v as Record<string, unknown>)[head], rest) };
}

export function parseCtrl(f: Frame): Ctrl {
  return asCtrl(ctrlJson(f));
}
