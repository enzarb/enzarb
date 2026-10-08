export type SessionEndKind = "closed" | "peer_closed" | "protocol" | "transport";

/** Why a session ended. */
export interface SessionEnd {
  kind: SessionEndKind;
  reason?: string;
}

export type SessionErrorCode =
  | "ended"
  | "handshake"
  | "channel_in_use"
  | "open_rejected"
  | "remote"
  | "channel_closed"
  | "channel_aborted"
  | "json_too_large";

/** Error from a session or channel operation. */
export class SessionError extends Error {
  constructor(
    readonly code: SessionErrorCode,
    message: string,
    readonly details: {
      end?: SessionEnd;
      /** For code "remote": the peer's `error` CTRL fields. */
      remote?: { code: string; message: string };
    } = {},
  ) {
    super(message);
    this.name = "SessionError";
  }

  static ended(end: SessionEnd): SessionError {
    return new SessionError("ended", `session ended: ${end.kind}${end.reason ? ` (${end.reason})` : ""}`, { end });
  }
}
