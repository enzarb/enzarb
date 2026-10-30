// A fake sandbox for the browser's mux websocket, served by Playwright's `routeWebSocket` (no server
// listens on the sandbox URL in CI). It is the server side of `Session.connect(ws, { client: 'ui' })`
// built on @enzarb/proto's own frame codec, and an ACP agent on the `acp` channel.
import type { Page, WebSocketRoute } from '@playwright/test';
import {
	AgentSideConnection,
	PROTOCOL_VERSION as ACP_PROTOCOL_VERSION,
	ndJsonStream,
	type Agent
} from '@agentclientprotocol/sdk';
import {
	ErrorCode,
	Feature,
	FrameType,
	SUPPORTED_VERSIONS,
	closeFrame,
	ctrlFrame,
	dataBin,
	decodeFrame,
	encodeFrame,
	negotiateVersion,
	parseCtrl,
	type Frame
} from '@enzarb/proto';
import { AGENT_REPLY } from './constants.ts';

export const SANDBOX_MUX_URL = /\/sandbox\/v1\/projects\/[^/]+\/mux/;

export interface FakeSandboxOptions {
	/** The agent's answer to every prompt. */
	reply?: string;
	/** Answer chat opens like a proxy whose user has no Claude token (`open_error{no_credential}`). */
	noCredential?: boolean;
	/** Files the sandbox serves to `file_read`, by path as requested (others answer `not_found`). */
	files?: Record<string, string>;
}

/** Serve the sandbox mux websocket of every project on `page`. */
export async function installFakeSandbox(page: Page, opts: FakeSandboxOptions = {}): Promise<void> {
	await page.routeWebSocket(SANDBOX_MUX_URL, (ws) =>
		serve(ws, opts.reply ?? AGENT_REPLY, opts.noCredential ?? false, opts.files ?? {})
	);
}

function serve(
	ws: WebSocketRoute,
	reply: string,
	noCredential: boolean,
	files: Record<string, string>
): void {
	const send = (f: Frame) => ws.send(Buffer.from(encodeFrame(f)));
	const fail = (code: string, message: string) => send(ctrlFrame({ type: 'error', code, message }));

	let welcomed = false;
	let acpChannel: number | null = null;
	let acpInbound: ReadableStreamDefaultController<Uint8Array> | null = null;

	function openAcp(channel: number): void {
		acpChannel = channel;
		const readable = new ReadableStream<Uint8Array>({ start: (c) => void (acpInbound = c) });
		const writable = new WritableStream<Uint8Array>({
			write: (chunk) => send(dataBin(channel, chunk))
		});
		send(ctrlFrame({ type: 'opened', channel }));
		new AgentSideConnection((conn) => fakeAgent(conn, reply), ndJsonStream(writable, readable));
	}

	function onCtrl(f: Frame): void {
		const msg = parseCtrl(f);
		switch (msg.type) {
			case 'hello': {
				const version = negotiateVersion(msg.versions, SUPPORTED_VERSIONS);
				if (version === null) {
					fail(ErrorCode.UnsupportedVersion, 'no common version');
					return;
				}
				welcomed = true;
				send(ctrlFrame({ type: 'welcome', version, features: [Feature.Acp, Feature.FileRead] }));
				return;
			}
			case 'open':
				if (noCredential) {
					send(
						ctrlFrame({
							type: 'open_error',
							channel: msg.channel,
							message: 'owner has no Claude credential',
							code: 'no_credential'
						})
					);
					return;
				}
				if (msg.kind !== 'acp' || acpChannel !== null) {
					send(
						ctrlFrame({
							type: 'open_error',
							channel: msg.channel,
							message: 'the fake sandbox only serves one acp channel',
							code: 'unsupported_kind'
						})
					);
					return;
				}
				openAcp(msg.channel);
				return;
			case 'file_read': {
				const content = files[msg.path];
				send(
					ctrlFrame(
						content === undefined
							? {
									type: 'file',
									id: msg.id,
									path: msg.path,
									code: 'not_found',
									message: 'no such file'
								}
							: { type: 'file', id: msg.id, path: msg.path, content }
					)
				);
				return;
			}
			default:
				fail(ErrorCode.Unimplemented, `the fake sandbox does not handle ${msg.type}`);
		}
	}

	ws.onMessage((message) => {
		if (typeof message === 'string') return;
		const f = decodeFrame(message);
		if (f.type === FrameType.Ctrl) {
			if (f.channel === 0 && (welcomed || parseCtrl(f).type === 'hello')) onCtrl(f);
			return;
		}
		if (f.channel !== acpChannel) return;
		if (f.type === FrameType.DataBin) acpInbound?.enqueue(f.payload);
		else if (f.type === FrameType.Close) {
			// Half-close from the client ends the agent: close our side too.
			acpInbound?.close();
			send(closeFrame(f.channel));
			acpChannel = null;
		}
		// Window grants need no handling: the replies stay far below the initial window.
	});
}

function fakeAgent(conn: AgentSideConnection, reply: string): Agent {
	const sessionId = 'e2e-session';
	return {
		async initialize() {
			return { protocolVersion: ACP_PROTOCOL_VERSION, agentCapabilities: { loadSession: false } };
		},
		async authenticate() {
			return {};
		},
		async newSession() {
			return { sessionId };
		},
		async prompt(params) {
			await conn.sessionUpdate({
				sessionId: params.sessionId,
				update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: reply } }
			});
			return { stopReason: 'end_turn' };
		},
		async cancel() {}
	};
}
