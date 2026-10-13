import { page } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import ClaudeSetup from './ClaudeSetup.svelte';

const h = vi.hoisted(() => ({
	configured: false,
	connections: [] as string[],
	removed: 0,
	refreshed: 0
}));
// Like the real query, `current` changes only when refresh() re-reads the server state: refresh
// bumps a reactive version so the component re-renders.
vi.mock('#lib/claude.remote.ts', async () => {
	const { SvelteMap } = await import('svelte/reactivity');
	return {
		claudeCredential: () => {
			const version = new SvelteMap([['v', 0]]);
			const served = { configured: h.configured };
			return {
				get current() {
					version.get('v');
					return { configured: served.configured, updatedAt: null };
				},
				error: undefined,
				refresh: async () => {
					h.refreshed++;
					served.configured = h.configured;
					version.set('v', (version.get('v') ?? 0) + 1);
				}
			};
		},
		removeClaudeCredential: async () => {
			h.removed++;
			h.configured = false;
		}
	};
});
vi.mock('#lib/terminal.remote.ts', () => ({
	terminalConnection: async (name: string) => {
		h.connections.push(name);
		return { url: 'ws://fake' };
	}
}));
vi.mock(
	'#lib/components/Terminal.svelte',
	() => import('#lib/components/__fixtures__/TerminalStub.svelte')
);

const web = { name: 'web', displayName: 'Web', ready: true };
const api = { name: 'api', displayName: 'API', ready: true };

describe('ClaudeSetup', () => {
	beforeEach(() => {
		h.configured = false;
		h.connections = [];
		h.removed = 0;
		h.refreshed = 0;
	});

	it('offers setup in the only ready sandbox when no token is set', async () => {
		render(ClaudeSetup, { sandboxes: [web] });
		await expect.element(page.getByText('Not configured')).toBeVisible();
		await expect.element(page.getByText(/sandbox of Web/)).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Remove' })).not.toBeInTheDocument();
		await page.getByRole('button', { name: 'Set up' }).click();
		await expect.element(page.getByText(/claude setup-token/)).toBeVisible();
	});

	it('removes the token without any sandbox connection, refreshing once', async () => {
		h.configured = true;
		render(ClaudeSetup, { sandboxes: [web] });
		await expect.element(page.getByText('Configured', { exact: true })).toBeVisible();
		await page.getByRole('button', { name: 'Remove' }).click();
		await expect.poll(() => h.removed).toBe(1);
		await expect.element(page.getByText('Not configured')).toBeVisible();
		expect(h.connections).toEqual([]);
		expect(h.refreshed).toBe(1);
		await expect.element(page.getByRole('alert')).not.toBeInTheDocument();
	});

	it('removes the token when every sandbox is stopped or there is none', async () => {
		h.configured = true;
		render(ClaudeSetup, { sandboxes: [{ ...web, ready: false }] });
		await page.getByRole('button', { name: 'Remove' }).click();
		await expect.poll(() => h.removed).toBe(1);
		expect(h.connections).toEqual([]);
	});

	it('says so when the user has no sandbox', async () => {
		render(ClaudeSetup, { sandboxes: [] });
		await expect.element(page.getByText(/Create a Project first/)).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Set up' })).not.toBeInTheDocument();
	});

	it('waits for a ready sandbox', async () => {
		render(ClaudeSetup, { sandboxes: [{ ...web, ready: false }] });
		await expect.element(page.getByText(/None of your sandboxes is ready/)).toBeVisible();
	});

	it('lets the user pick among several sandboxes', async () => {
		render(ClaudeSetup, { sandboxes: [api, web] });
		await expect.element(page.getByText('Sandbox')).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Set up' })).toBeVisible();
	});
});
