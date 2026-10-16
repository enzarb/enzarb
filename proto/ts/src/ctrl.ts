// CTRL messages (contracts section 4). Field names are the wire names, mirroring
// crates/enzarb-proto/src/ctrl.rs.

import { type Frame, FrameError, ctrlJson } from "./frame.js";
import { Secret, type SecretLike } from "./secret.js";

export { Secret, type SecretLike } from "./secret.js";

/** Known `credential_state.scope` values. Unknown values must be tolerated, so `scope` stays an open string. */
export const CredentialScope = { User: "user" } as const;

export type ClientKind = "ui" | "proxy" | "executor";
/**
 * Channel kinds. `acp` carries ACP's newline-delimited JSON-RPC as a byte stream. Only send it after
 * `welcome.features` contained `"acp"` (an old proxy closes the socket on an unknown kind). Received
 * kinds may be any string, so the type stays open.
 */
export type ChannelKind = "pty" | "acp" | (string & {});

/** Feature names advertised in `welcome.features`. */
/** `ClaudeUserToken` is a client feature (`hello.features`), sent only by sandbox-proxy. */
/** `Stats`: the server answers `stats_request` with `stats`; send the request only after seeing it. */
/** `Steps`: the server runs ADR-0022 steps (`step_*`, `session_event`); send `step_*` only after seeing it. */
export const Feature = { Acp: "acp", ClaudeUserToken: "claude_user_token", Stats: "stats", Steps: "steps" } as const;
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
export type CredentialKind = "claude_oauth";
/** Lifecycle of a step in the sandbox. */
export type StepPhase = "running" | "interrupted" | "finished" | "unknown";
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

/** A repository checked out for the step; the first is the primary workspace. */
export interface StepWorkspace {
  /** `owner/repo`. */
  repo: string;
  url: string;
  mode: "read" | "write";
  branch: string;
  base: string;
  push?: boolean;
  /** Short-lived token for fetching and pushing this repo. */
  token: SecretLike;
}

/** An agent runtime, resolved by the engine from the library's runtime file. */
export interface Runtime {
  name: string;
  install?: string[];
  /** `acp` or `stdio-json`. */
  protocol: string;
  command: string[];
  env?: Record<string, string>;
  capabilities?: unknown;
}

export type StepExec =
  | { kind: "agent"; runtime: Runtime; model?: string | null; prompt: string; result_schema?: unknown }
  | { kind: "command"; run: string; tools?: string[] };

/** A declared output file, read from the primary workspace after the step. */
export interface StepOutput {
  file: string;
  /** `markdown`, `json` (parsed) or `text`. */
  format?: string | null;
  optional?: boolean;
}

/** Body of `step_start`. */
export interface StepStart {
  run_id: string;
  attempt: number;
  task_id: string;
  workspaces: StepWorkspace[];
  exec: StepExec;
  outputs?: Record<string, StepOutput>;
  /** Named secrets as environment variables. */
  env?: Record<string, SecretLike>;
  /** `GH_TOKEN` for the step. */
  github_token?: SecretLike | null;
}

/** One captured, redacted session event; `seq` starts at 1 and is dense per run. */
export interface SessionEvent {
  run_id: string;
  seq: number;
  /** RFC 3339. */
  at: string;
  kind: string;
  payload: unknown;
  truncated?: boolean;
}

/** Git state of one write workspace after the step. */
export interface StepGit {
  repo: string;
  branch: string;
  head_sha: string;
  pushed: boolean;
}

/** Body of `step_finished` (and `step_state.result`). */
export interface StepFinished {
  run_id: string;
  /** See `StepExit`. */
  exit: string;
  outputs?: Record<string, unknown>;
  result?: unknown;
  git?: StepGit[];
  usage?: Usage | null;
  error?: string | null;
}

/**
 * Token usage and cost of an agent run. Every field is omitted when unknown
 * (absent means "not reported", never 0). `cost_usd` is as reported by the agent.
 */
export interface Usage {
  input_tokens?: number | null;
  output_tokens?: number | null;
  cache_read_input_tokens?: number | null;
  cache_creation_input_tokens?: number | null;
  models?: string[];
  cost_usd?: number | null;
}

/**
 * Raw cumulative sandbox counters (`stats` reply). Every field but `ts_ms` is omitted when it could
 * not be read (absent means unknown, never 0). Consumers compute rates from two snapshots.
 */
export interface SandboxStats {
  /** Unix time of the snapshot in milliseconds. */
  ts_ms: number;
  cpu_usage_usec?: number | null;
  cpu_user_usec?: number | null;
  cpu_system_usec?: number | null;
  /** `memory.current` in bytes (a gauge). */
  memory_bytes?: number | null;
  io_read_bytes?: number | null;
  io_write_bytes?: number | null;
  storage_used_bytes?: number | null;
  storage_total_bytes?: number | null;
}

export type Ctrl =
  | { type: "hello"; versions: number[]; client: ClientKind; features?: string[] | null }
  | { type: "welcome"; version: number; features?: string[] | null }
  | { type: "open"; channel: number; kind: ChannelKind; cols: number; rows: number; command?: string[] | null; repo?: string | null; resume?: string | null; claude_token?: SecretLike | null }
  | { type: "opened"; channel: number }
  | { type: "open_error"; channel: number; message: string; code?: string | null }
  | { type: "resize"; channel: number; cols: number; rows: number }
  | ({ type: "step_start" } & StepStart)
  | { type: "step_accepted"; run_id: string; state: StepPhase; existing: boolean }
  | { type: "step_steer"; run_id: string; text: string }
  | { type: "step_stop"; run_id: string }
  | { type: "step_resume"; run_id: string }
  | { type: "step_status"; run_id: string; after_seq?: number }
  | { type: "step_state"; run_id: string; state: StepPhase; result?: StepFinished | null }
  | ({ type: "session_event" } & SessionEvent)
  | ({ type: "step_finished" } & StepFinished)
  | { type: "credential_captured"; kind: CredentialKind; token?: SecretLike | null; legacy?: boolean | null }
  | { type: "credential_status" }
  | { type: "credential_state"; kind: CredentialKind; configured: boolean; scope?: string | null }
  | { type: "stats_request" }
  | ({ type: "stats" } & SandboxStats)
  /** Client -> server: clear the stored credential. */
  | { type: "credential_clear"; kind: CredentialKind }
  | { type: "error"; code: string; message: string; run_id?: string; channel?: number };

export type CtrlType = Ctrl["type"];

const TYPES: ReadonlySet<string> = new Set<CtrlType>([
  "hello",
  "welcome",
  "open",
  "opened",
  "open_error",
  "resize",
  "step_start",
  "step_accepted",
  "step_steer",
  "step_stop",
  "step_resume",
  "step_status",
  "step_state",
  "session_event",
  "step_finished",
  "credential_captured",
  "credential_status",
  "credential_state",
  "credential_clear",
  "stats_request",
  "stats",
  "error",
]);

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
  const o = v as Record<string, unknown>;
  if (t === "step_start") return wrapStepStart(o) as Ctrl;
  const field = t === "credential_captured" ? "token" : t === "open" ? "claude_token" : undefined;
  if (field !== undefined && typeof o[field] === "string") return { ...o, [field]: new Secret(o[field]) } as Ctrl;
  return v as Ctrl;
}

const wrap = (v: unknown): unknown => (typeof v === "string" ? new Secret(v) : v);

/** Wrap `step_start` tokens (workspace tokens, env values, `github_token`) in `Secret`s. */
function wrapStepStart(o: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = { ...o };
  if (Array.isArray(o.workspaces)) {
    out.workspaces = o.workspaces.map((w: unknown) =>
      typeof w === "object" && w !== null ? { ...w, token: wrap((w as Record<string, unknown>).token) } : w,
    );
  }
  if (typeof o.env === "object" && o.env !== null && !Array.isArray(o.env)) {
    out.env = Object.fromEntries(Object.entries(o.env).map(([k, v]) => [k, wrap(v)]));
  }
  if ("github_token" in o) out.github_token = wrap(o.github_token);
  return out;
}

export function parseCtrl(f: Frame): Ctrl {
  return asCtrl(ctrlJson(f));
}
