export * from "./frame.js";
export * from "./ctrl.js";
export * from "./flow.js";
export * from "./errors.js";
export { Channel, SEND_CHUNK, type ChannelClosed, type ChannelData, type ChannelHost } from "./channel.js";
export { ByteChannel } from "./bytes.js";
export {
  Session,
  type ConnectOptions,
  type CredentialInfo,
  type FileReply,
  type WebSocketLike,
} from "./session.js";

/** Current (and only) protocol version. */
export const PROTOCOL_VERSION = 1;
export const SUPPORTED_VERSIONS: readonly number[] = [1];

/** Highest version offered by the client that we also support. */
export function negotiateVersion(clientVersions: readonly number[], supported: readonly number[] = SUPPORTED_VERSIONS): number | null {
  const common = clientVersions.filter((v) => supported.includes(v));
  return common.length ? Math.max(...common) : null;
}
