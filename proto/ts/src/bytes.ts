import type { Channel, ChannelClosed } from "./channel.js";
import { CloseCode } from "./frame.js";

/**
 * A byte channel as browser Web Streams, for protocols that are plain byte streams on a mux channel
 * (the `acp` kind carries ACP's newline-delimited JSON-RPC). Shaped for `ndJsonStream(writable, readable)`
 * from `@agentclientprotocol/sdk`.
 *
 * Backpressure is end to end:
 * - `readable` pulls from the channel only when its consumer asks (high-water mark 0), and the channel
 *   returns credit to the peer as bytes are pulled, so a slow reader stalls the peer's writes.
 * - `writable` writes sequentially and each write waits for the peer's credit, so chunks keep their order.
 *
 * Closing `writable` half-closes the channel (CLOSE, normal); the peer's CLOSE ends `readable`.
 */
export class ByteChannel {
  readonly readable: ReadableStream<Uint8Array>;
  readonly writable: WritableStream<Uint8Array>;

  /** @internal */
  constructor(private readonly ch: Channel) {
    this.readable = new ReadableStream<Uint8Array>(
      {
        pull: async (controller) => {
          try {
            const d = await ch.read();
            if (d === null) controller.close();
            else if (d.data.length > 0) controller.enqueue(d.data);
          } catch (e) {
            controller.error(e);
          }
        },
        cancel: async () => {
          await ch.close(CloseCode.Cancelled, "cancelled").catch(() => undefined);
          // Keep returning credit so the peer is not left stalled mid-write.
          void (async () => {
            try {
              while ((await ch.read()) !== null);
            } catch {
              /* session ended */
            }
          })();
        },
      },
      { highWaterMark: 0 },
    );
    this.writable = new WritableStream<Uint8Array>({
      write: async (chunk) => {
        if (!(chunk instanceof Uint8Array)) throw new TypeError("ByteChannel.writable accepts Uint8Array chunks");
        if (chunk.length > 0) await ch.write(chunk);
      },
      close: () => ch.close(CloseCode.Normal, ""),
      abort: () => ch.close(CloseCode.Cancelled, "aborted").catch(() => undefined),
    });
  }

  /** The mux channel id. */
  get id(): number {
    return this.ch.id;
  }

  /**
   * Resolves (never rejects) when the peer closed the channel (`reason` is e.g. `idle`, `superseded`,
   * `shutdown`, `limit`, `exit:<n>`; tolerate any text) or the session ended. Bytes sent before the CLOSE
   * may still be unread on `readable`.
   */
  get closed(): Promise<ChannelClosed> {
    return this.ch.closed;
  }

  /** Half-close our side (CLOSE) without going through `writable`. */
  close(code: number = CloseCode.Normal, reason = ""): Promise<void> {
    return this.ch.close(code, reason);
  }
}
