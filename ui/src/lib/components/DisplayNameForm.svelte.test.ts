import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import DisplayNameForm from './DisplayNameForm.svelte';
import type { ProjectView } from '#lib/project-types.ts';

const view = (displayName = 'My project'): ProjectView => ({
	name: 'p',
	displayName,
	namespace: null,
	sandboxName: null,
	repositories: [],
	triggers: { issues: true, changeRequests: true, mentions: true },
	sandboxSettings: { cpu: null, memory: null, storage: null },
	conditions: [],
	ready: true,
	generation: 1,
	observedGeneration: 1,
	pendingChange: null
});

const editButton = () => page.getByRole('button', { name: 'Edit display name' });
const saveButton = () => page.getByRole('button', { name: 'Save display name' });
const input = () => page.getByRole('textbox', { name: 'Display name' });

describe('DisplayNameForm', () => {
	it('is read-only until Edit, and Cancel restores the value', async () => {
		render(DisplayNameForm, { project: view(), save: vi.fn() });
		await expect.element(page.getByText('My project')).toBeInTheDocument();
		await expect.element(input()).not.toBeInTheDocument();
		await editButton().click();
		await expect.element(input()).toHaveValue('My project');
		await expect.element(saveButton()).toBeDisabled();
		await input().fill('Changed');
		await page.getByRole('button', { name: 'Cancel' }).click();
		await expect.element(input()).not.toBeInTheDocument();
		await editButton().click();
		await expect.element(input()).toHaveValue('My project');
	});

	it('disables Save for empty input', async () => {
		render(DisplayNameForm, { project: view(), save: vi.fn() });
		await editButton().click();
		await input().fill('   ');
		await expect.element(saveButton()).toBeDisabled();
	});

	it('saves the trimmed value and leaves edit mode', async () => {
		const save = vi.fn(async () => ({ ok: true as const }));
		render(DisplayNameForm, { project: view(), save });
		await editButton().click();
		await input().fill('  Renamed  ');
		await saveButton().click();
		expect(save).toHaveBeenCalledWith('Renamed');
		await expect.element(input()).not.toBeInTheDocument();
	});

	it('shows the error and stays in edit mode when saving fails', async () => {
		const save = vi.fn(async () => ({ ok: false as const, message: 'Nope' }));
		render(DisplayNameForm, { project: view(), save });
		await editButton().click();
		await input().fill('Renamed');
		await saveButton().click();
		await expect.element(page.getByText('Nope')).toBeInTheDocument();
		await expect.element(input()).toBeInTheDocument();
	});
});
