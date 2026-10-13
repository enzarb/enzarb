import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import ProjectContextHost from '#lib/components/__fixtures__/ProjectContextHost.svelte';
import type { ProjectView } from '#lib/project-types.ts';
import ChatPage from './+page.svelte';

vi.mock('#lib/chat.remote.ts', () => ({ chatConnection: () => new Promise(() => {}) }));
vi.mock('$app/state', () => ({
	page: { params: { name: 'alpha' }, url: new URL('http://localhost/projects/alpha/chat') }
}));

const base = {
	name: 'alpha',
	displayName: 'Alpha Project',
	ready: true,
	pendingChange: null
} as unknown as ProjectView;

describe('chat tab', () => {
	it('connects with the checklist and no reconnect banner when nothing is pending', async () => {
		render(ProjectContextHost, { Child: ChatPage, project: base });
		await expect.element(page.getByText('Session token')).toBeVisible();
		await expect.element(page.getByText(/This chat will reconnect/)).not.toBeInTheDocument();
	});

	it('warns that the chat will reconnect while a sandbox change is pending', async () => {
		const project = { ...base, pendingChange: { phase: 'Queued' } } as unknown as ProjectView;
		render(ProjectContextHost, { Child: ChatPage, project });
		await expect.element(page.getByText(/This chat will reconnect/)).toBeVisible();
	});

	it('waits for the sandbox to boot', async () => {
		const project = { ...base, ready: false } as unknown as ProjectView;
		render(ProjectContextHost, { Child: ChatPage, project });
		await expect.element(page.getByText(/Chat is available once it is ready/)).toBeVisible();
	});
});
