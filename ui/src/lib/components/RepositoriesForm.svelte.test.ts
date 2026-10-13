import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import RepositoriesForm from './RepositoriesForm.svelte';
import type { ProjectView } from '#lib/project-types.ts';

const view = (urls: string[]): ProjectView => ({
	name: 'p',
	displayName: 'P',
	namespace: null,
	sandboxName: null,
	repositories: urls.map((url) => ({ url, connector: 'github', defaultBranch: 'main' })),
	triggers: { issues: true, changeRequests: true, mentions: true },
	sandboxSettings: { cpu: null, memory: null, storage: null },
	conditions: [],
	ready: true,
	generation: 1,
	observedGeneration: 1,
	pendingChange: null
});

const choices = [
	{ repo: 'github:enzarb/enzarb', private: false },
	{ repo: 'github:enzarb/other', private: false }
];
const box = (name: string) => page.getByRole('checkbox', { name, exact: true });
const saveButton = () => page.getByRole('button', { name: 'Save repositories' });

describe('RepositoriesForm', () => {
	it('swaps an archived repository for a new one and shows the checklist', async () => {
		const save = vi.fn(async () => ({ ok: true as const }));
		render(RepositoriesForm, {
			project: view(['https://github.com/enzarb/enzarb-ui']),
			choices,
			save
		});
		await expect.element(box('enzarb/enzarb-ui')).toBeChecked();
		await expect
			.element(page.getByText('No longer in your GitHub list (archived or access removed)'))
			.toBeInTheDocument();
		await expect.element(saveButton()).toBeDisabled();
		await box('enzarb/enzarb').click();
		await box('enzarb/enzarb-ui').click();
		await saveButton().click();
		expect(save).toHaveBeenCalledWith(['github:enzarb/enzarb']);
		await expect
			.element(page.getByRole('list', { name: 'Repository change progress' }))
			.toBeInTheDocument();
	});

	it('needs at least one repository', async () => {
		render(RepositoriesForm, {
			project: view(['https://github.com/enzarb/enzarb']),
			choices,
			save: vi.fn()
		});
		await box('enzarb/enzarb').click();
		await expect.element(page.getByText('Select at least one repository.')).toBeInTheDocument();
		await expect.element(saveButton()).toBeDisabled();
	});

	it('disables repositories held by another Project and shows a refusal', async () => {
		const save = vi.fn(async () => ({ ok: false as const, message: 'nope' }));
		render(RepositoriesForm, {
			project: view(['https://github.com/enzarb/enzarb']),
			choices,
			taken: { 'github:enzarb/other': 'q' },
			save
		});
		await expect.element(box('enzarb/other')).toBeDisabled();
		await expect.element(page.getByText('In "q"')).toBeInTheDocument();
	});
});
