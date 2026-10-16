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
const editButton = () => page.getByRole('button', { name: 'Edit repositories' });

describe('RepositoriesForm', () => {
	it('offers Refresh while editing and explains which repositories are listed', async () => {
		const refresh = vi.fn(async () => {});
		render(RepositoriesForm, {
			project: view(['https://github.com/enzarb/enzarb']),
			choices,
			save: vi.fn(),
			refresh
		});
		await editButton().click();
		await expect
			.element(
				page.getByText(/repositories the enzarb GitHub App can access where you have write access/)
			)
			.toBeVisible();
		await page.getByRole('button', { name: 'Refresh repositories' }).click();
		expect(refresh).toHaveBeenCalledOnce();
	});

	it('is read-only until Edit, and Cancel discards changes', async () => {
		render(RepositoriesForm, {
			project: view(['https://github.com/enzarb/enzarb-ui']),
			choices,
			save: vi.fn()
		});
		await expect
			.element(page.getByRole('link', { name: 'enzarb/enzarb-ui' }))
			.toHaveAttribute('href', 'https://github.com/enzarb/enzarb-ui');
		await expect.element(page.getByText(/No longer in your GitHub list/)).toBeInTheDocument();
		await expect.element(box('enzarb/enzarb-ui')).not.toBeInTheDocument();
		await editButton().click();
		await box('enzarb/enzarb').click();
		await page.getByRole('button', { name: 'Cancel' }).click();
		await expect.element(box('enzarb/enzarb')).not.toBeInTheDocument();
		await expect
			.element(page.getByRole('link', { name: 'enzarb/enzarb', exact: true }))
			.not.toBeInTheDocument();
		await editButton().click();
		await expect.element(box('enzarb/enzarb')).not.toBeChecked();
	});

	it('returns to the read-only view with the new list after saving', async () => {
		const save = vi.fn(async () => ({ ok: true as const }));
		const r = render(RepositoriesForm, {
			project: view(['https://github.com/enzarb/enzarb']),
			choices,
			save
		});
		await editButton().click();
		await box('enzarb/other').click();
		await saveButton().click();
		await r.rerender({
			project: view(['https://github.com/enzarb/enzarb', 'https://github.com/enzarb/other'])
		});
		await expect.element(page.getByRole('link', { name: 'enzarb/other' })).toBeInTheDocument();
		await expect.element(editButton()).toBeInTheDocument();
		await expect.element(saveButton()).not.toBeInTheDocument();
		await expect
			.element(page.getByRole('list', { name: 'Repository change progress' }))
			.toBeInTheDocument();
	});

	it('shows no Edit button to non-owners', async () => {
		render(RepositoriesForm, {
			project: view(['https://github.com/enzarb/enzarb']),
			choices,
			save: vi.fn(),
			canEdit: false
		});
		await expect
			.element(page.getByText('Only the project owner can change these settings.'))
			.toBeInTheDocument();
		await expect.element(editButton()).not.toBeInTheDocument();
	});

	it('swaps an archived repository for a new one and shows the checklist', async () => {
		const save = vi.fn(async () => ({ ok: true as const }));
		render(RepositoriesForm, {
			project: view(['https://github.com/enzarb/enzarb-ui']),
			choices,
			save
		});
		await editButton().click();
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
		await editButton().click();
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
		await editButton().click();
		await expect.element(box('enzarb/other')).toBeDisabled();
		await expect.element(page.getByText('In "q"')).toBeInTheDocument();
	});
});
