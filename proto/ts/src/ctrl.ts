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
/**
 * `RunResults`: the server pushes `run_finished` to the session that started or subscribed to a run,
 * answers `run_status` with `run_state.result`, and accepts `run_start` without a callback.
 */
export const Feature = { Acp: "acp", ClaudeUserToken: "claude_user_token", Stats: "stats", RunResults: "run_results", Prepare: "prepare" } as const;
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
export type RunPhase = "queued" | "running" | "succeeded" | "failed" | "cancelled";
export type RunOutcome = "succeeded" | "failed" | "cancelled";

export const ErrorCode = {
  UnsupportedVersion: "unsupported_version",
  UnknownMessage: "unknown_message",
  ProtocolViolation: "protocol_violation",
  FlowControl: "flow_control",
  Unauthorized: "unauthorized",
  UnknownChannel: "unknown_channel",
  UnknownRun: "unknown_run",
  Internal: "internal",
  /** The peer does not implement the request (e.g. `run_*` without a run manager). */
  Unimplemented: "unimplemented",
} as const;

/** `run_start.retarget`: rebase the child branch onto `onto` and push with a lease. */
export interface Retarget {
  head_ref: string;
  /** Branch to rebase onto (the default branch). */
  onto: string;
  /** Parent head sha the child was cut from. */
  old_base: string;
  /** Child head sha the push lease expects. */
  expected_sha: string;
  /** Branch names that must never be rewritten. */
  protected?: string[];
}

export interface RunStartFields {
  run_id: string;
  repo_url: string;
  base_ref: string;
  branch: string;
  push: boolean;
  clone_token: string;
  prompt: string;
  timeout_secs: number;
  /** Deprecated: omit when the server advertises `run_results` (old servers require it). */
  callback_url?: string;
  /** Deprecated: bearer token for `callback_url`; omit with it. */
  callback_token?: string;
  /** Client-chosen channel for the transcript (DATA_JSON), or null. */
  log_channel: number | null;
  /** Optional read-only token for same-Project git deps; never for push. */
  deps_token?: string;
  /** Optional read:packages token for private npm packages (NODE_AUTH_TOKEN); never persisted. */
  npm_token?: string;
  /** Optional short-lived read-only token (`actions: read`) for `gh` CI log reads (GH_TOKEN, agent process only); never persisted. */
  ci_read_token?: string;
  /** Optional per-user Claude token. Held as a `Secret` (redacted in logs); sent as a plain string. */
  claude_token?: SecretLike;
  /** When set, a retarget run instead of an agent run. */
  retarget?: Retarget;
  /** When set, a graph segment run (ADR-0020); `prompt` is ignored. */
  graph?: GraphSegment;
}

/** `run_start.graph`: a workflow graph segment for sandbox-init to walk. */
export interface GraphSegment {
  /** The pinned workflow definition (JSON form of the YAML). */
  definition: unknown;
  /** Node id to start at. */
  start: string;
  /** Outputs of nodes run so far, by node id. */
  outputs?: Record<string, unknown>;
  /** Visit counts so far, by node id. */
  visits?: Record<string, number>;
  /** Rendered prompt text of `agent` nodes, by node id. */
  prompts?: Record<string, string>;
}

/** `RunFinished.graph`: where a segment run stopped and every node's output. */
export interface GraphResult {
  stopped_at: string;
  outputs?: Record<string, unknown>;
  visits?: Record<string, number>;
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

/** Final run result: body of `run_finished` and `run_state.result` (and, deprecated, of the callback POST). */
export interface RunFinished {
  run_id: string;
  outcome: RunOutcome;
  exit_code?: number | null;
  summary: string;
  branch: string;
  head_sha: string;
  pushed: boolean;
  usage?: Usage | null;
  /** Result of a graph segment run. */
  graph?: GraphResult | null;
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

/** `prepared` reply: best-effort run preparation results; empty fields are omitted. */
export interface Prepared {
  run_id: string;
  merge_base_sha?: string | null;
  changed_files?: string[];
  /** The true number of changed files (may exceed `changed_files.length`). */
  changed_files_total?: number;
  affected?: string[];
}

export type Ctrl =
  | { type: "hello"; versions: number[]; client: ClientKind; features?: string[] | null }
  | { type: "welcome"; version: number; features?: string[] | null }
  | { type: "open"; channel: number; kind: ChannelKind; cols: number; rows: number; command?: string[] | null; repo?: string | null; resume?: string | null; claude_token?: SecretLike | null }
  | { type: "opened"; channel: number }
  | { type: "open_error"; channel: number; message: string; code?: string | null }
  | { type: "resize"; channel: number; cols: number; rows: number }
  | ({ type: "run_start" } & RunStartFields)
  | { type: "run_accepted"; run_id: string; state: RunPhase; existing: boolean }
  | { type: "run_status"; run_id: string }
  | { type: "run_state"; run_id: string; state: RunPhase; exit_code?: number | null; summary?: string | null; usage?: Usage | null; result?: RunFinished | null }
  | ({ type: "run_finished" } & RunFinished)
  | { type: "run_subscribe"; run_ids: string[] }
  | { type: "run_cancel"; run_id: string }
  | { type: "credential_captured"; kind: CredentialKind; token?: SecretLike | null; legacy?: boolean | null }
  | { type: "credential_status" }
  | { type: "credential_state"; kind: CredentialKind; configured: boolean; scope?: string | null }
  | { type: "stats_request" }
  | ({ type: "stats" } & SandboxStats)
  | { type: "prepare_request"; run_id: string; repo_url: string; base_ref: string; branch: string; clone_token: SecretLike }
  | ({ type: "prepared" } & Prepared)
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
  "run_start",
  "run_accepted",
  "run_status",
  "run_state",
  "run_finished",
  "run_subscribe",
  "run_cancel",
  "credential_captured",
  "credential_status",
  "credential_state",
  "credential_clear",
  "stats_request",
  "stats",
  "prepare_request",
  "prepared",
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
  const field =
    t === "credential_captured"
      ? "token"
      : t === "open" || t === "run_start"
        ? "claude_token"
        : t === "prepare_request"
          ? "clone_token"
          : undefined;
  if (field !== undefined && typeof o[field] === "string") return { ...o, [field]: new Secret(o[field]) } as Ctrl;
  return v as Ctrl;
}

export function parseCtrl(f: Frame): Ctrl {
  return asCtrl(ctrlJson(f));
}
