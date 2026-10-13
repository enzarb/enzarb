// Per-channel credit-based flow control state machine, one instance per channel
// per endpoint. Mirrors crates/enzarb-proto/src/flow.rs. No I/O, no clocks.

import { INITIAL_WINDOW, MAX_CREDIT } from "./frame.js";

export type FlowErrorCode =
  | "insufficient_credit"
  | "local_closed"
  | "already_closed"
  | "overrun"
  | "data_after_close"
  | "credit_overflow"
  | "release_underflow";

export class FlowError extends Error {
  constructor(
    readonly code: FlowErrorCode,
    message?: string,
  ) {
    super(message ?? code);
    this.name = "FlowError";
  }
}

export class FlowControl {
  private sendCredit_: number;
  private recvGranted_: number;
  private recvUnreleased = 0;
  private pendingGrant = 0;
  private readonly grantThreshold: number;
  private localClosed = false;
  private remoteClosed = false;

  /** `initialSend`: credit the peer gave us. `initialRecv`: credit we give the peer. */
  constructor(initialSend = INITIAL_WINDOW, initialRecv = INITIAL_WINDOW) {
    this.sendCredit_ = Math.min(initialSend, MAX_CREDIT);
    this.recvGranted_ = Math.min(initialRecv, MAX_CREDIT);
    this.grantThreshold = Math.max(Math.floor(initialRecv / 2), 1);
  }

  // ---- sending ----

  get sendCredit(): number {
    return this.sendCredit_;
  }

  /** Largest chunk <= want that may be sent now. */
  sendable(want: number): number {
    return this.localClosed ? 0 : Math.min(this.sendCredit_, want);
  }

  onSend(len: number): void {
    if (this.localClosed) throw new FlowError("local_closed", "send after local close");
    if (len > this.sendCredit_) {
      throw new FlowError("insufficient_credit", `send of ${len} bytes exceeds credit ${this.sendCredit_}`);
    }
    this.sendCredit_ -= len;
  }

  /** Peer granted credit. Ignored after local close. */
  onWindow(credit: number): void {
    if (this.localClosed) return;
    const total = this.sendCredit_ + credit;
    if (total > MAX_CREDIT) throw new FlowError("credit_overflow", "window grant would push credit past 2^31-1");
    this.sendCredit_ = total;
  }

  closeLocal(): void {
    if (this.localClosed) throw new FlowError("already_closed", "local side already closed");
    this.localClosed = true;
    this.sendCredit_ = 0;
  }

  // ---- receiving ----

  get recvGranted(): number {
    return this.recvGranted_;
  }

  onData(len: number): void {
    if (this.remoteClosed) throw new FlowError("data_after_close", "data after peer close");
    if (len > this.recvGranted_) {
      throw new FlowError("overrun", `peer sent ${len} bytes with only ${this.recvGranted_} granted`);
    }
    this.recvGranted_ -= len;
    this.recvUnreleased += len;
  }

  /** The application consumed `len` bytes. Returns a WINDOW credit to send, once enough accumulated. */
  release(len: number): number | null {
    if (len > this.recvUnreleased) throw new FlowError("release_underflow", "released more bytes than were received");
    this.recvUnreleased -= len;
    this.pendingGrant += len;
    if (this.remoteClosed || this.pendingGrant < this.grantThreshold) return null;
    return this.takeGrant();
  }

  /** Force out any pending grant. */
  flushGrant(): number | null {
    return this.remoteClosed ? null : this.takeGrant();
  }

  private takeGrant(): number | null {
    if (this.pendingGrant === 0) return null;
    const g = this.pendingGrant;
    this.pendingGrant = 0;
    this.recvGranted_ += g;
    return g;
  }

  onCloseRemote(): void {
    this.remoteClosed = true;
  }

  get isLocalClosed(): boolean {
    return this.localClosed;
  }

  get isRemoteClosed(): boolean {
    return this.remoteClosed;
  }

  get isClosed(): boolean {
    return this.localClosed && this.remoteClosed;
  }
}
