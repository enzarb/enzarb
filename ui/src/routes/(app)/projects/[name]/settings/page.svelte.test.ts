import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { behavior } from '#lib/components/__fixtures__/fake-project-remote.svelte.ts';
import ProjectContextHost from '#lib/components/__fixtures__/ProjectContextHost.svelte';
import type { ProjectView } from '#lib/project-types.ts';
import Settings from './+page.svelte';

vi.mock(
	'#lib/projects.remote.ts',
	() => import('#lib/components/__fixtures__/fake-project-remote.svelte.ts')
);
vi.mock('$app/state', () => ({
	page: { params: { name: 'alpha' }, url: new URL('http://localhost/projects/alpha/settings') }
}));
vi.mock('$app/navigation', () => ({ goto: async () => {} }));

const project = {
	name: 'alpha',
	displayName: 'Alpha Project',
	sandboxName: 'sb',
	conditions: [],
	sandboxSettings: { cpu: null, memory: null, storage: null },
	pendingChange: null,
	repositories: [],
	triggers: { issues: true, changeRequests: false, mentions: false }
} as unknown as ProjectView;

describe('settings tab', () => {
	afterEach(() => {
		behavior.settingsError = undefined;
	});

	it('renders the sections', async () => {
		const screen = await render(ProjectContextHost, { Child: Settings, project });
		await expect.element(screen.getByText('Danger zone')).toBeInTheDocument();
		await expect.element(screen.getByText('Concurrency')).toBeInTheDocument();
	});

	it('has no Claude section', async () => {
		const screen = await render(ProjectContextHost, { Child: Settings, project });
		await expect.element(screen.getByText('Danger zone')).toBeInTheDocument();
		expect(screen.container.textContent).not.toContain('Claude');
	});

	it('keeps the page when the settings query fails', async () => {
		behavior.settingsError = new Error('settings down');
		const screen = await render(ProjectContextHost, { Child: Settings, project });
		await expect.element(screen.getByText('Danger zone')).toBeInTheDocument();
	});
});
