import { page as browser } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { ProjectView } from '#lib/project-types.ts';

const h = vi.hoisted(() => ({
	url: new URL('http://localhost/projects/alpha'),
	project: undefined as unknown,
	goto: undefined as unknown
}));

vi.mock('$app/state', () => ({
	page: {
		get url() {
			return h.url;
		},
		params: { name: 'alpha' }
	}
}));
vi.mock('$app/navigation', () => ({
	goto: (...a: unknown[]) => (h.goto as (...a: unknown[]) => unknown)(...a)
}));
vi.mock('#lib/projects.remote.ts', () => ({
	getProject: () => ({ current: h.project, refresh: async () => {} }),
	deleteProject: vi.fn(async () => {})
}));
vi.mock('#lib/terminal.remote.ts', () => ({
	getProjectSettings: () => ({
		current: { claudeTokenConfigured: true, claudeConfiguredBy: 'ruckc' }
	})
}));
vi.mock(
	'#lib/components/CredentialStatus.svelte',
	() => import('#lib/components/__fixtures__/CredentialStatusStub.svelte')
);
vi.mock(
	'#lib/components/Terminal.svelte',
	() => import('#lib/components/__fixtures__/TerminalStub.svelte')
);

import Page from './+page.svelte';

const view = (over: Partial<ProjectView> = {}): ProjectView => ({
	name: 'alpha',
	displayName: 'Alpha',
	namespace: 'ns',
	sandboxName: 'sb',
	repositories: [],
	triggers: { issues: true, changeRequests: false, mentions: false },
	conditions: [{ type: 'Ready', status: 'True' }],
	ready: true,
	...over
});

describe('project page', () => {
	beforeEach(() => {
		h.url = new URL('http://localhost/projects/alpha');
		h.project = view();
		h.goto = vi.fn();
	});

	it('hides the boot checklist when the project is ready', async () => {
		render(Page);
		await expect.element(browser.getByRole('tab', { name: 'Overview' })).toBeVisible();
		await expect.element(browser.getByText('Boot progress')).not.toBeInTheDocument();
	});

	it('shows the boot checklist while booting', async () => {
		h.project = view({ ready: false, conditions: [] });
		render(Page);
		await expect.element(browser.getByText('Boot progress')).toBeVisible();
	});

	it('keeps the checklist for a failed boot', async () => {
		h.project = view({
			ready: false,
			conditions: [{ type: 'SandboxReady', status: 'False', reason: 'SandboxFailed' }]
		});
		render(Page);
		await expect.element(browser.getByText('Boot progress')).toBeVisible();
	});

	it('renders three tabs with Overview selected by default', async () => {
		render(Page);
		for (const name of ['Overview', 'Terminal', 'Settings'])
			await expect.element(browser.getByRole('tab', { name })).toBeVisible();
		await expect
			.element(browser.getByRole('tab', { name: 'Overview' }))
			.toHaveAttribute('aria-selected', 'true');
		await expect.element(browser.getByRole('heading', { name: 'Repositories' })).toBeVisible();
	});

	it('falls back to Overview for an unknown tab', async () => {
		h.url = new URL('http://localhost/projects/alpha?tab=nope');
		render(Page);
		await expect
			.element(browser.getByRole('tab', { name: 'Overview' }))
			.toHaveAttribute('aria-selected', 'true');
	});

	it('puts Delete project in Settings and keeps the delete flow', async () => {
		h.url = new URL('http://localhost/projects/alpha?tab=settings');
		render(Page);
		await expect
			.element(browser.getByRole('tab', { name: 'Settings' }))
			.toHaveAttribute('aria-selected', 'true');
		await browser.getByRole('button', { name: 'Delete project' }).click();
		await expect.element(browser.getByRole('button', { name: 'Confirm delete' })).toBeVisible();
		await browser.getByRole('button', { name: 'Confirm delete' }).click();
		await vi.waitFor(() => expect(h.goto).toHaveBeenCalledWith('/projects'));
	});

	it('writes the selected tab to the URL', async () => {
		render(Page);
		await browser.getByRole('tab', { name: 'Settings' }).click();
		await vi.waitFor(() => expect(h.goto).toHaveBeenCalled());
		const [url, opts] = (h.goto as ReturnType<typeof vi.fn>).mock.calls[0];
		expect(String(url)).toContain('tab=settings');
		expect(opts).toMatchObject({ replace: true, reset: false });
	});

	it('mounts the terminal, filling the region, only when ready', async () => {
		h.url = new URL('http://localhost/projects/alpha?tab=terminal');
		render(Page);
		await expect.element(browser.getByTestId('terminal')).toHaveAttribute('data-fill', 'true');
	});

	it('does not mount the terminal while booting', async () => {
		h.url = new URL('http://localhost/projects/alpha?tab=terminal');
		h.project = view({ ready: false, conditions: [] });
		render(Page);
		await expect.element(browser.getByText(/still booting/)).toBeVisible();
		await expect.element(browser.getByTestId('terminal')).not.toBeInTheDocument();
	});
});
