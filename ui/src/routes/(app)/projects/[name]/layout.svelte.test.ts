import { page as browser } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { ProjectView } from '#lib/project-types.ts';

const h = vi.hoisted(() => ({
	path: '/projects/alpha',
	project: undefined as unknown
}));

vi.mock('$app/state', () => ({
	page: {
		get url() {
			return new URL(`http://localhost${h.path}`);
		},
		params: { name: 'alpha' }
	}
}));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
vi.mock('#lib/projects.remote.ts', () => ({
	getProject: () => ({ current: h.project, refresh: async () => {} }),
	deleteProject: vi.fn(async () => {}),
	setMaxRuns: vi.fn(async () => {}),
	setSandboxResources: vi.fn(async () => {}),
	setDisplayName: vi.fn(async () => ({ ok: true })),
	setRepositories: vi.fn(async () => ({ ok: true })),
	repositoryChoices: () => ({
		current: { repositories: [], taken: {}, problem: null },
		refresh: async () => {}
	})
}));
vi.mock('#lib/work.remote.ts', () => ({
	listWorkItems: () => ({ current: [], refresh: async () => {} })
}));
vi.mock('#lib/claude.remote.ts', () => ({
	claudeCredential: () => ({ current: { configured: true, updatedAt: null } })
}));
vi.mock(
	'#lib/components/Terminal.svelte',
	() => import('#lib/components/__fixtures__/TerminalStub.svelte')
);

import Layout from './+layout.svelte';
import Overview from './+page.svelte';
import Settings from './settings/+page.svelte';
import Terminal from './terminal/+page.svelte';
import LayoutHost from '#lib/components/__fixtures__/LayoutHost.svelte';

const view = (over: Partial<ProjectView> = {}): ProjectView => ({
	name: 'alpha',
	displayName: 'Alpha',
	namespace: 'ns',
	sandboxName: 'sb',
	repositories: [],
	triggers: { issues: true, changeRequests: false, mentions: false },
	sandboxSettings: { cpu: null, memory: null, storage: null },
	conditions: [{ type: 'Ready', status: 'True' }],
	ready: true,
	generation: 1,
	observedGeneration: 1,
	pendingChange: null,
	...over
});

const mount = (Child: unknown) => render(LayoutHost, { Layout, Child } as never);

describe('project layout and tabs', () => {
	beforeEach(() => {
		h.path = '/projects/alpha';
		h.project = view();
	});

	it('links the three tabs and marks Overview current by default', async () => {
		mount(Overview);
		const nav = browser.getByRole('navigation', { name: 'Project sections' });
		await expect
			.element(nav.getByRole('link', { name: 'Overview' }))
			.toHaveAttribute('href', '/projects/alpha');
		await expect
			.element(nav.getByRole('link', { name: 'Terminal' }))
			.toHaveAttribute('href', '/projects/alpha/terminal');
		await expect
			.element(nav.getByRole('link', { name: 'Settings' }))
			.toHaveAttribute('href', '/projects/alpha/settings');
		await expect
			.element(nav.getByRole('link', { name: 'Metrics' }))
			.toHaveAttribute('href', '/projects/alpha/metrics');
		await expect
			.element(nav.getByRole('link', { name: 'Overview' }))
			.toHaveAttribute('aria-current', 'page');
		await expect
			.element(nav.getByRole('link', { name: 'Terminal' }))
			.not.toHaveAttribute('aria-current');
		await expect.element(browser.getByRole('heading', { name: 'Repositories' })).toBeVisible();
	});

	it('renders the title, subtext and tabs in one header row', async () => {
		mount(Overview);
		const headers = document.querySelectorAll('header');
		const header = document.querySelector('header.ez-project-header');
		expect(header).not.toBeNull();
		expect(document.querySelector('.ez-page-header')).toBeNull();
		expect(headers.length).toBe(1);
		expect(header!.querySelector('h1')?.textContent).toBe('Alpha');
		expect(header!.textContent).toContain('sandbox');
		expect(header!.querySelector('nav[aria-label="Project sections"]')).not.toBeNull();
		expect(document.querySelector('.ez-project-view > .ez-tabnav-panel')).not.toBeNull();
	});

	it('marks the tab of the current route', async () => {
		h.path = '/projects/alpha/settings';
		mount(Settings);
		await expect
			.element(browser.getByRole('link', { name: 'Settings' }))
			.toHaveAttribute('aria-current', 'page');
		await expect
			.element(browser.getByRole('link', { name: 'Overview' }))
			.not.toHaveAttribute('aria-current');
	});

	it('keeps the Metrics tab current on a nested metrics route', async () => {
		h.path = '/projects/alpha/metrics/timing';
		mount(Settings);
		await expect
			.element(browser.getByRole('link', { name: 'Metrics' }))
			.toHaveAttribute('aria-current', 'page');
		await expect
			.element(browser.getByRole('link', { name: 'Overview' }))
			.not.toHaveAttribute('aria-current');
	});

	it('shows the pending-change banner once, on every tab', async () => {
		h.project = view({
			pendingChange: { since: '2026-01-01T00:00:00Z', reason: 'resources' } as never
		});
		h.path = '/projects/alpha/terminal';
		mount(Terminal);
		await expect.element(browser.getByTestId('pending-change')).toBeVisible();
		expect(document.querySelectorAll('[data-testid="pending-change"]').length).toBe(1);
	});

	it('hides the boot checklist when the project is ready', async () => {
		mount(Overview);
		await expect.element(browser.getByText('Boot progress')).not.toBeInTheDocument();
	});

	it('shows the boot checklist while booting', async () => {
		h.project = view({ ready: false, conditions: [] });
		mount(Overview);
		await expect.element(browser.getByText('Boot progress')).toBeVisible();
	});

	it('keeps the checklist for a failed boot', async () => {
		h.project = view({
			ready: false,
			conditions: [{ type: 'SandboxReady', status: 'False', reason: 'SandboxFailed' }]
		});
		mount(Overview);
		await expect.element(browser.getByText('Boot progress')).toBeVisible();
	});

	it('puts Delete project in Settings and keeps the delete flow', async () => {
		h.path = '/projects/alpha/settings';
		const { goto } = await import('$app/navigation');
		mount(Settings);
		await browser.getByRole('button', { name: 'Delete project' }).click();
		await expect.element(browser.getByRole('button', { name: 'Confirm delete' })).toBeVisible();
		await browser.getByRole('button', { name: 'Confirm delete' }).click();
		await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/projects'));
	});

	it('mounts the terminal, filling the region, only when ready', async () => {
		h.path = '/projects/alpha/terminal';
		mount(Terminal);
		await expect.element(browser.getByTestId('terminal')).toHaveAttribute('data-fill', 'true');
		await expect.element(browser.getByText(/for emergencies/)).not.toBeInTheDocument();
	});

	it('does not mount the terminal while booting', async () => {
		h.path = '/projects/alpha/terminal';
		h.project = view({ ready: false, conditions: [] });
		mount(Terminal);
		await expect.element(browser.getByText(/still booting/)).toBeVisible();
		await expect.element(browser.getByTestId('terminal')).not.toBeInTheDocument();
	});
});
