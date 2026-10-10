// Test double for `#lib/terminal.remote.ts`. Like SvelteKit's client `command()`, it bumps a
// `$state` counter synchronously, so calling it in an effect's sync part reads and writes a signal.
let pending = $state(0);
export const pendingCount = () => pending;
export const calls: string[] = [];
export const syncs: { name: string; release: () => void }[] = [];
/** When set, terminalConnection resolves (so the component proceeds to sync); otherwise it hangs. */
export const behavior = { resolve: false };

export function terminalConnection(name: string): Promise<{ url: string }> {
	pending++;
	calls.push(name);
	return behavior.resolve ? Promise.resolve({ url: 'ws://fake' }) : new Promise(() => {});
}

export function syncClaudeCredential(args: { name: string }): Promise<void> {
	return new Promise((release) => syncs.push({ name: args.name, release: () => release() }));
}
