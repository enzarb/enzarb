// CTRL messages (contracts section 4). Field names are the wire names, mirroring
// crates/enzarb-proto/src/ctrl.rs.

import { type Frame, FrameError, ctrlJson } from "./frame.js";

export type ClientKind = "ui" | "proxy" | "executor";
/**
 * Channel kinds. `acp` carries ACP's newline-delimited JSON-RPC as a byte stream. Only send it after
 * `welcome.features` contained `"acp"` (an old proxy closes the socket on an unknown kind). Received
 * kinds may be any string, so the type stays open.
 */
export type ChannelKind = "pty" | "acp" | (string & {});

/** Feature names advertised in `welcome.features`. */
export const Feature = { Acp: "acp" } as const;
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

export interface RunStartFields {
  run_id: string;
  repo_url: string;
  base_ref: string;
  branch: string;
  push: boolean;
  clone_token: string;
  prompt: string;
  timeout_secs: number;
  callback_url: string;
  callback_token: string;
  /** Client-chosen channel for the transcript (DATA_JSON), or null. */
  log_channel: number | null;
  /** Optional read-only token for same-Project git deps; never for push. */
  deps_token?: string;
  /** Optional read:packages token for private npm packages (NODE_AUTH_TOKEN); never persisted. */
  npm_token?: string;
}

/** Body POSTed to `callback_url` (not a CTRL message). */
export interface RunFinished {
  run_id: string;
  outcome: RunOutcome;
  exit_code?: number | null;
  summary: string;
  branch: string;
  head_sha: string;
  pushed: boolean;
}

export type Ctrl =
  | { type: "hello"; versions: number[]; client: ClientKind }
  | { type: "welcome"; version: number; features?: string[] | null }
  | { type: "open"; channel: number; kind: ChannelKind; cols: number; rows: number; command?: string[] | null; repo?: string | null; resume?: string | null }
  | { type: "opened"; channel: number }
  | { type: "open_error"; channel: number; message: string; code?: string | null }
  | { type: "resize"; channel: number; cols: number; rows: number }
  | ({ type: "run_start" } & RunStartFields)
  | { type: "run_accepted"; run_id: string; state: RunPhase; existing: boolean }
  | { type: "run_status"; run_id: string }
  | { type: "run_state"; run_id: string; state: RunPhase; exit_code?: number | null; summary?: string | null }
  | { type: "run_cancel"; run_id: string }
  | { type: "credential_captured"; kind: CredentialKind }
  | { type: "credential_status" }
  | { type: "credential_state"; kind: CredentialKind; configured: boolean }
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
  "run_cancel",
  "credential_captured",
  "credential_status",
  "credential_state",
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
  return v as Ctrl;
}

export function parseCtrl(f: Frame): Ctrl {
  return asCtrl(ctrlJson(f));
}
