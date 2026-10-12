import {
	ClientSideConnection,
	PROTOCOL_VERSION,
	RequestError,
	ndJsonStream,
	type Client,
	type InitializeResponse
} from '@agentclientprotocol/sdk';
import { MAX_PENDING, MAX_PROMPT_BYTES } from './types.ts';
import type { PermissionOutcome } from './permissions.ts';

/** The only extension notification the browser trusts. */
export const EXT_PERMISSION_DENIED = '_enzarb/permission_denied';
/** `session/set_mode` is limited to these. */
export type SessionMode = 'default' | 'plan';

/** The byte streams of an `openAcp` channel (a `ByteChannel`). */
export interface AcpStreams {
	readable: ReadableStream<Uint8Array>;
	writable: WritableStream<Uint8Array>;
}

export interface AcpHandlers {
	sessionUpdate(sessionId: string, update: unknown): void;
	/** Must settle with `cancelled` when the connection goes away. */
	requestPermission(req: {
		sessionId: string;
		toolCall: unknown;
		options: unknown;
		meta: unknown;
	}): Promise<PermissionOutcome>;
	permissionDenied(params: unknown): void;
}

export interface AcpClient {
	initialize(): Promise<InitializeResponse>;
	newSession(): Promise<string>;
	/** `session/load`; resolves when the agent finished replaying. */
	loadSession(sessionId: string): Promise<void>;
	prompt(sessionId: string, text: string): Promise<string>;
	/** `session/cancel` notification. */
	cancel(sessionId: string): Promise<void>;
	setMode(sessionId: string, mode: SessionMode): Promise<void>;
	/** Resolves when the connection ended (either side). */
	readonly closed: Promise<void>;
}

export class AcpLimitError extends Error {}

/** Where the agent works inside the sandbox. */
export const DEFAULT_CWD = '/workspace';

export function createAcpClient(
	stream: AcpStreams,
	handlers: AcpHandlers,
	opts: { cwd?: string } = {}
): AcpClient {
	const cwd = opts.cwd ?? DEFAULT_CWD;
	const reject = (method: string) => () => {
		throw RequestError.methodNotFound(method);
	};
	// No fs/terminal methods are implemented: the SDK would answer an absent handler with an empty
	// result, so each one throws method-not-found instead.
	const toClient: Client = {
		async requestPermission(p) {
			const out = await handlers.requestPermission({
				sessionId: p.sessionId,
				toolCall: p.toolCall,
				options: p.options,
				meta: p._meta
			});
			return { outcome: out };
		},
		sessionUpdate(p) {
			handlers.sessionUpdate(p.sessionId, p.update);
		},
		readTextFile: reject('fs/read_text_file'),
		writeTextFile: reject('fs/write_text_file'),
		createTerminal: reject('terminal/create'),
		terminalOutput: reject('terminal/output'),
		releaseTerminal: reject('terminal/release'),
		waitForTerminalExit: reject('terminal/wait_for_exit'),
		killTerminal: reject('terminal/kill'),
		extMethod(method) {
			throw RequestError.methodNotFound(method);
		},
		extNotification(method, params) {
			if (method === EXT_PERMISSION_DENIED) handlers.permissionDenied(params);
		}
	};
	const conn = new ClientSideConnection(
		() => toClient,
		ndJsonStream(stream.writable, stream.readable)
	);

	let inFlight = 0;
	async function track<T>(run: () => Promise<T>): Promise<T> {
		if (inFlight >= MAX_PENDING) throw new AcpLimitError('too many requests in flight');
		inFlight++;
		try {
			return await run();
		} finally {
			inFlight--;
		}
	}

	return {
		initialize: () =>
			track(() =>
				conn.initialize({
					protocolVersion: PROTOCOL_VERSION,
					clientCapabilities: {},
					clientInfo: { name: 'enzarb-ui', version: '0.0.1' }
				})
			),
		newSession: () => track(async () => (await conn.newSession({ cwd, mcpServers: [] })).sessionId),
		loadSession: (sessionId) =>
			track(async () => {
				await conn.loadSession({ sessionId, cwd, mcpServers: [] });
			}),
		prompt: (sessionId, text) => {
			if (new TextEncoder().encode(text).length > MAX_PROMPT_BYTES) {
				return Promise.reject(new AcpLimitError('prompt is too large'));
			}
			return track(
				async () => (await conn.prompt({ sessionId, prompt: [{ type: 'text', text }] })).stopReason
			);
		},
		cancel: (sessionId) => conn.cancel({ sessionId }),
		setMode: (sessionId, mode) => {
			if (mode !== 'default' && mode !== 'plan') {
				return Promise.reject(new AcpLimitError('unsupported mode'));
			}
			return track(async () => {
				await conn.setSessionMode({ sessionId, modeId: mode });
			});
		},
		closed: conn.closed
	};
}
