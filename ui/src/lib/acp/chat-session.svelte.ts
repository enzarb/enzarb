import { createChatConnection, type ConnectionDeps, type ConnectionState } from './connection.ts';
import type { PermissionChoice } from './types.ts';

/** Page visibility as the connection wants it. */
export const browserVisibility: ConnectionDeps['visibility'] = {
	state: () => (document.visibilityState === 'visible' ? 'visible' : 'hidden'),
	onChange(cb) {
		document.addEventListener('visibilitychange', cb);
		return () => document.removeEventListener('visibilitychange', cb);
	}
};

/** Timers and randomness from the browser. */
export const browserTimers = {
	setTimeout: (fn: () => void, ms: number) => setTimeout(fn, ms),
	clearTimeout: (h: unknown) => clearTimeout(h as ReturnType<typeof setTimeout>),
	random: () => Math.random()
};

/**
 * The reactive wrapper the chat page binds to: a `$state` copy of the connection's state plus its
 * commands. All behaviour lives in `createChatConnection`; call `dispose()` on unmount.
 */
export function createChatSession(deps: ConnectionDeps) {
	let state = $state.raw<ConnectionState>(undefined as unknown as ConnectionState);
	const conn = createChatConnection(deps, (s) => {
		state = s;
	});
	state = conn.state();
	return {
		get state() {
			return state;
		},
		start: () => conn.start(),
		reconnect: () => conn.reconnect(),
		send: (text: string) => conn.send(text),
		newChat: () => conn.newChat(),
		stop: () => conn.stop(),
		answer: (id: string, choice: PermissionChoice, epoch: number) => conn.answer(id, choice, epoch),
		dispose: () => conn.dispose()
	};
}
