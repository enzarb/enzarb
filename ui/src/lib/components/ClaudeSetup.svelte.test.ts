import { page } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import ClaudeSetup from './ClaudeSetup.svelte';

const h = vi.hoisted(() => ({
	configured: false,
	connections: [] as string[],
	cleared: 0
}));
vi.mock('#lib/claude.remote.ts', () => ({
	claudeCredential: () => ({
		get current() {
			return { configured: h.configured, updatedAt: null };
		},
		error: undefined,
		refresh: async () => {}
	})
}));
vi.mock('#lib/terminal.remote.ts', () => ({
	terminalConnection: async (name: string) => {
		h.connections.push(name);
		return { url: 'ws://fake' };
	}
}));
vi.mock('@enzarb/proto', () => ({
	Session: {
		connect: async () => ({
			credentialClear: () => {
				h.cleared++;
				h.configured = false;
			},
			close() {}
		})
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
		h.cleared = 0;
	});

	it('offers setup in the only ready sandbox when no token is set', async () => {
		render(ClaudeSetup, { sandboxes: [web] });
		await expect.element(page.getByText('Not configured')).toBeVisible();
		await expect.element(page.getByText(/sandbox of Web/)).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Remove' })).not.toBeInTheDocument();
		await page.getByRole('button', { name: 'Set up' }).click();
		await expect.element(page.getByText(/claude setup-token/)).toBeVisible();
	});

	it('shows Configured with Remove, which sends credential_clear', async () => {
		h.configured = true;
		render(ClaudeSetup, { sandboxes: [web] });
		await expect.element(page.getByText('Configured', { exact: true })).toBeVisible();
		await page.getByRole('button', { name: 'Remove' }).click();
		await expect.poll(() => h.cleared).toBe(1);
		expect(h.connections).toEqual(['web']);
		await expect.element(page.getByRole('button', { name: 'Remove' })).toBeEnabled();
		await expect.element(page.getByRole('alert')).not.toBeInTheDocument();
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
