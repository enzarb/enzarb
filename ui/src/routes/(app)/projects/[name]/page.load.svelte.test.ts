import { error } from '@sveltejs/kit';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { behavior, counts } from '#lib/components/__fixtures__/fake-project-remote.svelte.ts';
import Page from './+page.svelte';

vi.mock(
	'#lib/projects.remote.ts',
	() => import('#lib/components/__fixtures__/fake-project-remote.svelte.ts')
);
vi.mock('#lib/terminal.remote.ts', async () => {
	const fake = await import('#lib/components/__fixtures__/fake-project-remote.svelte.ts');
	return {
		getProjectSettings: fake.getProjectSettings,
		terminalConnection: () => new Promise(() => {}),
		syncClaudeCredential: () => new Promise(() => {})
	};
});
vi.mock('$app/state', () => ({
	page: { params: { name: 'alpha' }, url: new URL('http://localhost/projects/alpha') }
}));
vi.mock('$app/navigation', () => ({ goto: async () => {} }));

const project = {
	name: 'alpha',
	displayName: 'Alpha Project',
	sandboxName: 'sb',
	conditions: [],
	repositories: [],
	triggers: { issues: true, changeRequests: false, mentions: false }
};
const notFound = () => {
	try {
		error(404, 'Project not found');
	} catch (e) {
		return e;
	}
};

describe('project page', () => {
	afterEach(() => {
		behavior.project = undefined;
		behavior.projectError = undefined;
		behavior.settingsError = undefined;
		behavior.refreshError = undefined;
		counts.refresh = 0;
	});

	it('renders the project, not "Project not found"', async () => {
		behavior.project = project;
		const screen = await render(Page);
		await expect.element(screen.getByText('Alpha Project')).toBeInTheDocument();
		expect(screen.container.textContent).not.toContain('Project not found');
	});

	it('shows "Project not found" for a real 404', async () => {
		behavior.projectError = notFound();
		const screen = await render(Page);
		await expect.element(screen.getByText('Project not found').first()).toBeInTheDocument();
	});

	it('shows a retryable error for a non-404 failure', async () => {
		behavior.projectError = new Error('boom');
		const screen = await render(Page);
		await expect.element(screen.getByText('Could not load the project')).toBeInTheDocument();
		expect(screen.container.textContent).not.toContain('Project not found');
		behavior.projectError = undefined;
		behavior.project = project;
		await screen.getByRole('button', { name: 'Retry' }).click();
		await expect.element(screen.getByText('Alpha Project')).toBeInTheDocument();
	});

	it('keeps the page when the settings query fails', async () => {
		behavior.project = project;
		behavior.settingsError = new Error('settings down');
		const screen = await render(Page);
		await expect.element(screen.getByText('Alpha Project')).toBeInTheDocument();
	});

	it('keeps the page when a poll refresh rejects', async () => {
		behavior.project = project;
		behavior.refreshError = new Error('poll failed');
		const screen = await render(Page);
		await new Promise((r) => setTimeout(r, 2300));
		expect(counts.refresh).toBeGreaterThan(0);
		await expect.element(screen.getByText('Alpha Project')).toBeInTheDocument();
	});
});
