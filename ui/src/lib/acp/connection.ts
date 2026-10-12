import { CloseReason, Feature, MAX_RESUME_LEN, OpenErrorCode } from '@enzarb/proto';
import {
	completeChatStep,
	failChatStep,
	initialChatProgress,
	type ChatProgress,
	type ChatStepId
} from '../chat-steps.ts';
import { createAcpClient, type AcpClient, type AcpHandlers, type AcpStreams } from './client.ts';
import {
	PermissionRegistry,
	policyHint,
	sanitizeOptions,
	type PermissionOutcome
} from './permissions.ts';
import {
	addNotice,
	addPermission,
	addUserMessage,
	applyPermissionDenied,
	applyUpdate,
	cancelPending,
	resolvePermission,
	toolView
} from './reducer.ts';
import { initialChat, type ChatState, type PermissionChoice } from './types.ts';

export const BACKOFF_BASE_MS = 1_000;
export const BACKOFF_MAX_MS = 30_000;
export const MAX_RETRIES = 6;
/** The sandbox needs up to ~20 s to tear down the old agent before `opened`. */
export const OPEN_TIMEOUT_MS = 30_000;

export const NOT_RESTORED_TEXT = 'Previous conversation could not be restored; started a new one';

/** The slice of proto's `ByteChannel` the connection uses. */
export interface AcpChannel extends AcpStreams {
	closed: Promise<{ code: number | null; reason: string }>;
	close(code?: number, reason?: string): Promise<void>;
}

/** The slice of proto's `Session` the connection uses. */
export interface AcpSession {
	features: ReadonlySet<string>;
	openAcp(opts?: { repo?: string; resume?: string }): Promise<AcpChannel>;
	closed?: Promise<unknown>;
}

export type ConnectReport = (step: Extract<ChatStepId, 'token' | 'connect' | 'handshake'>) => void;

export interface ConnectionDeps {
	/** Fetch a token, open the websocket and handshake; call `report` as each step finishes. */
	connect(report: ConnectReport): Promise<AcpSession>;
	repo?: string;
	setTimeout(fn: () => void, ms: number): unknown;
	clearTimeout(handle: unknown): void;
	random(): number;
	visibility: {
		state(): 'visible' | 'hidden';
		/** Subscribe to visibility changes; returns the unsubscribe function. */
		onChange(cb: () => void): () => void;
	};
	createClient?: (streams: AcpStreams, handlers: AcpHandlers) => AcpClient;
	openTimeoutMs?: number;
	/** ACP session id saved by an earlier page load; offered as `open.resume` on the first open. */
	initialSessionId?: string;
	/** Called with the id once `session/load` or `session/new` succeeded, and with null on New chat. */
	onSessionId?: (id: string | null) => void;
}

export type Phase =
	| 'idle'
	| 'connecting'
	| 'ready'
	| 'unavailable'
	| 'failed'
	| 'disconnected'
	| 'superseded'
	| 'limit'
	| 'restarting'
	| 'reconnecting';

/** Button the status area offers for the current phase. */
export type ConnectionAction = 'retry' | 'reconnect' | 'take_over' | null;

export interface ConnectionState {
	phase: Phase;
	progress: ChatProgress;
	message: string;
	action: ConnectionAction;
	/** Bumped on every (re)connect; permission answers must carry the epoch they were rendered in. */
	epoch: number;
	attempt: number;
	sessionId: string | null;
	/** A prompt turn is running. */
	busy: boolean;
	chat: ChatState;
	/** Last send failure, if any. */
	error: string;
}

interface Failure {
	message: string;
	action: ConnectionAction;
}

const GENERIC_OPEN_FAILURE: Failure = { message: 'Chat could not be started', action: 'retry' };

const OPEN_FAILURES: Record<string, Failure> = {
	[OpenErrorCode.ChatDisabled]: { message: 'Chat is disabled for this project', action: null },
	[OpenErrorCode.ChatLimit]: {
		message: 'A chat is already open on this connection',
		action: 'retry'
	},
	[OpenErrorCode.NoCredential]: {
		message: 'No Claude credential is connected for this project',
		action: 'retry'
	},
	[OpenErrorCode.RepoNotCloned]: {
		message: 'The repository is not cloned in the sandbox yet',
		action: 'retry'
	},
	[OpenErrorCode.UnsupportedKind]: { message: 'This sandbox does not support chat', action: null },
	[OpenErrorCode.InvalidRequest]: { message: 'The chat request was invalid', action: null }
};

/** Fixed, never server-supplied, text for an `open_error` code; unknown codes get a generic one. */
export function describeOpenError(code: unknown): Failure {
	return typeof code === 'string' && Object.hasOwn(OPEN_FAILURES, code)
		? OPEN_FAILURES[code]
		: GENERIC_OPEN_FAILURE;
}

export type CloseDecision =
	| {
			kind: 'wait';
			phase: 'disconnected' | 'superseded' | 'limit';
			message: string;
			action: ConnectionAction;
	  }
	| { kind: 'restart' }
	| { kind: 'backoff' };

/** What to do after the sandbox closed the channel with `reason`. */
export function decideClose(reason: string | undefined | null): CloseDecision {
	switch (reason) {
		case CloseReason.Idle:
			return {
				kind: 'wait',
				phase: 'disconnected',
				message: 'Disconnected after being idle',
				action: 'reconnect'
			};
		case CloseReason.Superseded:
			return {
				kind: 'wait',
				phase: 'superseded',
				message: 'Chat opened in another tab',
				action: 'take_over'
			};
		case CloseReason.Limit:
			return {
				kind: 'wait',
				phase: 'limit',
				message: 'This chat reached its session limit',
				action: 'reconnect'
			};
		case CloseReason.Shutdown:
			return { kind: 'restart' };
		default:
			return { kind: 'backoff' };
	}
}

/** Delay before retry `attempt` (0-based): 1 s doubling to 30 s, shaved by up to 20 % of jitter. */
export function backoffDelay(attempt: number, random: () => number): number {
	const base = Math.min(BACKOFF_MAX_MS, BACKOFF_BASE_MS * 2 ** attempt);
	return Math.round(base * (1 - 0.2 * random()));
}

export interface ChatConnection {
	state(): ConnectionState;
	/** First connect. */
	start(): void;
	/** Manual Reconnect / Retry / Take over. */
	reconnect(): void;
	/** Send a prompt; after an idle close this reconnects first. */
	send(text: string): void;
	/** Drop the conversation and its saved id, then open a fresh session. */
	newChat(): void;
	/** Cancel the running turn and every pending permission. */
	stop(): void;
	answer(id: string, choice: PermissionChoice, epoch: number): void;
	dispose(): void;
}

export function createChatConnection(
	deps: ConnectionDeps,
	onChange: (s: ConnectionState) => void = () => {}
): ChatConnection {
	const openTimeout = deps.openTimeoutMs ?? OPEN_TIMEOUT_MS;
	const registry = new PermissionRegistry();
	let s: ConnectionState = {
		phase: 'idle',
		progress: initialChatProgress,
		message: '',
		action: null,
		epoch: 0,
		attempt: 0,
		sessionId: deps.initialSessionId || null,
		busy: false,
		chat: initialChat,
		error: ''
	};
	let session: AcpSession | null = null;
	let sessionEnded = false;
	let channel: AcpChannel | null = null;
	let channelClosed: Promise<unknown> | null = null;
	let client: AcpClient | null = null;
	let retryTimer: unknown = null;
	let unwatchVisibility: (() => void) | null = null;
	let queued: string | null = null;
	let permSeq = 0;
	let disposed = false;

	function set(patch: Partial<ConnectionState>): void {
		s = { ...s, ...patch };
		onChange(s);
	}
	const setChat = (chat: ChatState) => set({ chat });
	const stale = (epoch: number) => disposed || epoch !== s.epoch;

	function clearWaits(): void {
		if (retryTimer !== null) deps.clearTimeout(retryTimer);
		retryTimer = null;
		unwatchVisibility?.();
		unwatchVisibility = null;
	}

	/** Every pending permission becomes cancelled and its resolver settles. */
	function cancelAllPermissions(): void {
		registry.cancelAll();
		setChat(cancelPending(s.chat));
	}

	function handlersFor(epoch: number): AcpHandlers {
		return {
			sessionUpdate(sessionId, update) {
				if (stale(epoch) || sessionId !== s.sessionId) return;
				setChat(applyUpdate(s.chat, update));
			},
			permissionDenied(params) {
				if (stale(epoch)) return;
				setChat(applyPermissionDenied(s.chat, params, s.sessionId));
			},
			requestPermission(req) {
				const cancelled: PermissionOutcome = { outcome: 'cancelled' };
				if (stale(epoch) || req.sessionId !== s.sessionId) return Promise.resolve(cancelled);
				const tool = toolView(req.toolCall);
				const { options, duplicate } = sanitizeOptions(req.options);
				if (!tool) return Promise.resolve(cancelled);
				const id = `perm-${++permSeq}`;
				if (duplicate) {
					setChat(addPermission(s.chat, { id, toolCall: tool, options: [], status: 'cancelled' }));
					return Promise.resolve(cancelled);
				}
				setChat(addPermission(s.chat, { id, toolCall: tool, options, hint: policyHint(req.meta) }));
				return new Promise<PermissionOutcome>((resolve) => {
					registry.register(id, epoch, options, resolve);
				});
			}
		};
	}

	function fail(
		step: ChatStepId | undefined,
		message: string,
		patch: Partial<ConnectionState>
	): void {
		queued = null;
		set({ progress: failChatStep(s.progress, message, step), message, ...patch });
	}

	function scheduleRetry(why: string, phase: Phase = 'reconnecting'): void {
		if (disposed) return;
		if (s.attempt >= MAX_RETRIES) {
			set({ phase: 'failed', message: 'Could not reconnect to the chat', action: 'reconnect' });
			return;
		}
		const delay = backoffDelay(s.attempt, deps.random);
		set({ phase, message: why, action: null, attempt: s.attempt + 1 });
		retryTimer = deps.setTimeout(() => {
			retryTimer = null;
			void run();
		}, delay);
	}

	function onChannelClosed(epoch: number, reason: string | undefined): void {
		if (stale(epoch)) return;
		channel = null;
		client = null;
		cancelAllPermissions();
		queued = null;
		set({ busy: false });
		const d = decideClose(reason);
		if (d.kind === 'wait') {
			set({ phase: d.phase, message: d.message, action: d.action });
		} else if (d.kind === 'backoff') {
			scheduleRetry('Connection lost; reconnecting');
		} else if (deps.visibility.state() === 'visible') {
			scheduleRetry('Sandbox restarting', 'restarting');
		} else {
			set({ phase: 'restarting', message: 'Sandbox restarting', action: null });
			unwatchVisibility = deps.visibility.onChange(() => {
				if (deps.visibility.state() !== 'visible' || stale(epoch)) return;
				unwatchVisibility?.();
				unwatchVisibility = null;
				scheduleRetry('Sandbox restarting', 'restarting');
			});
		}
	}

	async function openWithTimeout(
		sess: AcpSession,
		resume: string | undefined
	): Promise<AcpChannel | 'timeout'> {
		const opts: { repo?: string; resume?: string } = {};
		if (deps.repo) opts.repo = deps.repo;
		if (resume) opts.resume = resume;
		const opening = sess.openAcp(opts);
		let timer: unknown;
		const timeout = new Promise<'timeout'>((resolve) => {
			timer = deps.setTimeout(() => resolve('timeout'), openTimeout);
		});
		try {
			const r = await Promise.race([opening, timeout]);
			if (r === 'timeout') {
				// A channel that opens after we gave up must not stay live.
				opening.then(
					(c) => void c.close().catch(() => {}),
					() => {}
				);
			}
			return r;
		} finally {
			deps.clearTimeout(timer);
		}
	}

	async function run(): Promise<void> {
		clearWaits();
		cancelAllPermissions();
		const epoch = s.epoch + 1;
		set({
			epoch,
			phase: 'connecting',
			progress: initialChatProgress,
			message: '',
			action: null,
			busy: false,
			error: ''
		});

		// Never hold two channels: wait for the old one to be gone before opening another.
		if (channel) {
			const old = channelClosed;
			void channel.close().catch(() => {});
			channel = null;
			client = null;
			await old;
			if (stale(epoch)) return;
		}

		const report: ConnectReport = (step) => {
			if (!stale(epoch)) set({ progress: completeChatStep(s.progress, step) });
		};

		if (!session || sessionEnded) {
			session = null;
			try {
				const sess = await deps.connect(report);
				if (stale(epoch)) return;
				session = sess;
				sessionEnded = false;
				void sess.closed?.then(() => {
					if (session === sess) sessionEnded = true;
				});
			} catch {
				if (stale(epoch)) return;
				fail(undefined, 'Could not connect to the sandbox', {});
				scheduleRetry('Could not connect; retrying');
				return;
			}
		}
		for (const step of ['token', 'connect', 'handshake'] as const) report(step);

		if (!session.features.has(Feature.Acp)) {
			fail('agent', 'Chat is not available', { phase: 'unavailable', action: null });
			return;
		}

		const resume =
			s.sessionId && new TextEncoder().encode(s.sessionId).length <= MAX_RESUME_LEN
				? s.sessionId
				: undefined;
		let ch: AcpChannel;
		try {
			const r = await openWithTimeout(session, resume);
			if (stale(epoch)) {
				if (r !== 'timeout') void r.close().catch(() => {});
				return;
			}
			if (r === 'timeout') {
				fail('agent', 'Agent did not start', { phase: 'failed', action: 'retry' });
				return;
			}
			ch = r;
		} catch (e) {
			if (stale(epoch)) return;
			const code = (e as { openCode?: unknown } | null)?.openCode;
			const isOpenError = e !== null && typeof e === 'object' && 'openCode' in e;
			if (isOpenError) {
				const f = describeOpenError(code);
				fail('agent', f.message, { phase: 'failed', action: f.action });
			} else {
				fail('agent', 'Could not start the agent', {});
				scheduleRetry('Could not start the agent; retrying');
			}
			return;
		}
		set({ progress: completeChatStep(s.progress, 'agent') });
		channel = ch;
		channelClosed = ch.closed;
		void ch.closed.then((c) => onChannelClosed(epoch, c.reason));

		const c = (deps.createClient ?? createAcpClient)(ch, handlersFor(epoch));
		client = c;
		// A broken stream with the channel still open: close it, the close handler decides what next.
		void c.closed.then(
			() => ch.close().catch(() => {}),
			() => ch.close().catch(() => {})
		);
		try {
			const init = await c.initialize();
			if (stale(epoch)) return;
			set({ progress: completeChatStep(s.progress, 'initialize') });
			await openSession(c, epoch, resume, init.agentCapabilities?.loadSession === true);
		} catch {
			if (stale(epoch)) return;
			fail(undefined, 'The agent did not respond', {});
			void ch.close().catch(() => {});
			return;
		}
		if (stale(epoch)) return;
		set({
			progress: completeChatStep(s.progress, 'session'),
			phase: 'ready',
			message: '',
			action: null,
			attempt: 0
		});
		if (queued !== null) {
			const text = queued;
			queued = null;
			doSend(text);
		}
	}

	async function openSession(
		c: AcpClient,
		epoch: number,
		resume: string | undefined,
		canLoad: boolean
	): Promise<void> {
		if (resume && canLoad) {
			// Replay rebuilds the transcript; keep the old one in case the load fails.
			const old = s.chat;
			set({ chat: initialChat, sessionId: resume });
			try {
				await c.loadSession(resume);
				if (!stale(epoch)) deps.onSessionId?.(resume);
				return;
			} catch {
				if (stale(epoch)) return;
				set({ chat: old, sessionId: null });
			}
		}
		const id = await c.newSession();
		if (stale(epoch)) return;
		set({ sessionId: id });
		deps.onSessionId?.(id);
		if (resume) setChat(addNotice(s.chat, 'not_restored', NOT_RESTORED_TEXT));
	}

	function doSend(text: string): void {
		const c = client;
		const sessionId = s.sessionId;
		if (!c || !sessionId || s.busy || s.phase !== 'ready') return;
		const epoch = s.epoch;
		set({ busy: true, error: '', chat: addUserMessage(s.chat, text) });
		c.prompt(sessionId, text).then(
			() => {
				if (!stale(epoch)) set({ busy: false });
			},
			() => {
				if (!stale(epoch)) set({ busy: false, error: 'The message could not be sent' });
			}
		);
	}

	return {
		state: () => s,
		start() {
			if (s.phase === 'idle') void run();
		},
		reconnect() {
			if (disposed) return;
			set({ attempt: 0 });
			void run();
		},
		send(text) {
			if (disposed || !text.trim()) return;
			if (s.phase === 'ready') doSend(text);
			else if (s.phase === 'disconnected') {
				queued = text;
				set({ attempt: 0 });
				void run();
			}
		},
		newChat() {
			if (disposed) return;
			queued = null;
			cancelAllPermissions();
			set({ sessionId: null, chat: initialChat, attempt: 0 });
			deps.onSessionId?.(null);
			void run();
		},
		stop() {
			if (s.busy && client && s.sessionId) void client.cancel(s.sessionId).catch(() => {});
			cancelAllPermissions();
		},
		answer(id, choice, epoch) {
			const status = registry.answer(id, choice, epoch);
			if (status === 'allowed' || status === 'denied')
				setChat(resolvePermission(s.chat, id, status));
		},
		dispose() {
			if (disposed) return;
			cancelAllPermissions();
			disposed = true;
			clearWaits();
			void channel?.close().catch(() => {});
			channel = null;
			client = null;
		}
	};
}
