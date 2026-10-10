import { flushSync } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import CredentialStatus from './CredentialStatus.svelte';
import { behavior, calls, pendingCount, syncs } from './__fixtures__/fake-remote.svelte.ts';

vi.mock('#lib/terminal.remote.ts', () => import('./__fixtures__/fake-remote.svelte.ts'));
vi.mock('@enzarb/proto', () => ({
	Session: {
		connect: async () => ({ credentialStatus: async () => ({ configured: true }), close() {} })
	}
}));

const settle = () => new Promise((r) => setTimeout(r, 200));

describe('CredentialStatus', () => {
	afterEach(() => {
		calls.length = 0;
		syncs.length = 0;
		behavior.resolve = false;
	});

	it('requests the terminal connection once per project, not in a loop (#43)', async () => {
		const screen = await render(CredentialStatus, { project: 'alpha', cached: false });
		await settle();
		expect(calls).toEqual(['alpha']);
		expect(pendingCount()).toBe(1);

		await screen.rerender({ project: 'beta' });
		flushSync();
		await settle();
		expect(calls).toEqual(['alpha', 'beta']);
	});

	it('does not report an old project answer after the project changed', async () => {
		behavior.resolve = true;
		const onchecked = vi.fn();
		const screen = await render(CredentialStatus, { project: 'alpha', cached: false, onchecked });
		await settle();
		expect(syncs.map((s) => s.name)).toEqual(['alpha']);

		await screen.rerender({ project: 'beta', onchecked });
		flushSync();
		await settle();
		syncs[0].release();
		await settle();
		expect(onchecked).not.toHaveBeenCalled();

		syncs[1].release();
		await settle();
		expect(onchecked).toHaveBeenCalledExactlyOnceWith(true);
	});
});
