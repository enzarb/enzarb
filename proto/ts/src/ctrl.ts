// CTRL messages (contracts section 4). Field names are the wire names, mirroring
// crates/enzarb-proto/src/ctrl.rs.

import { type Frame, FrameError, ctrlJson } from "./frame.js";

export type ClientKind = "ui" | "proxy" | "executor";
export type ChannelKind = "pty";
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
  | { type: "welcome"; version: number }
  | { type: "open"; channel: number; kind: ChannelKind; cols: number; rows: number; command?: string[] | null }
  | { type: "opened"; channel: number }
  | { type: "open_error"; channel: number; message: string }
  | { type: "resize"; channel: number; cols: number; rows: number }
  | ({ type: "run_start" } & RunStartFields)
  | { type: "run_accepted"; run_id: string; state: RunPhase; existing: boolean }
  | { type: "run_status"; run_id: string }
  | { type: "run_state"; run_id: string; state: RunPhase; exit_code?: number | null; summary?: string | null }
  | { type: "run_cancel"; run_id: string }
  | { type: "credential_captured"; kind: CredentialKind }
  | { type: "error"; code: string; message: string };

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
