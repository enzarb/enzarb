import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import SandboxResourcesForm from './SandboxResourcesForm.svelte';
import { POD_SHAPE_QUEUED_COPY } from '#lib/pending-change.ts';
import type { ProjectView } from '#lib/project-types.ts';

const view = (over: Partial<ProjectView> = {}): ProjectView => ({
	name: 'p',
	displayName: 'P',
	namespace: null,
	sandboxName: null,
	repositories: [],
	triggers: { issues: true, changeRequests: true, mentions: true },
	sandboxSettings: { cpu: null, memory: null, storage: null },
	conditions: [],
	ready: true,
	generation: 1,
	observedGeneration: 1,
	pendingChange: null,
	...over
});

const cpu = () => page.getByRole('spinbutton', { name: 'CPU (cores)' });
const memory = () => page.getByRole('textbox', { name: 'Memory' });
const storage = () => page.getByRole('textbox', { name: 'Storage' });
const saveButton = () => page.getByRole('button', { name: 'Save resources' });

describe('SandboxResourcesForm', () => {
	it('shows unset limits and the 20Gi storage default', async () => {
		render(SandboxResourcesForm, { project: view(), save: vi.fn() });
		await expect.element(page.getByText('not set (no limit)').first()).toBeInTheDocument();
		await expect.element(page.getByText('20Gi (default)')).toBeInTheDocument();
		await expect.element(saveButton()).toBeDisabled();
	});

	it('shows the current values', async () => {
		render(SandboxResourcesForm, {
			project: view({ sandboxSettings: { cpu: 8, memory: '16Gi', storage: '40Gi' } }),
			save: vi.fn()
		});
		await expect.element(cpu()).toHaveValue(8);
		await expect.element(memory()).toHaveValue('16');
		await expect.element(storage()).toHaveValue('40');
	});

	it('flags invalid values inline and keeps Save disabled', async () => {
		render(SandboxResourcesForm, { project: view(), save: vi.fn() });
		await cpu().fill('2.5');
		await expect.element(cpu()).toHaveAttribute('aria-invalid', 'true');
		await cpu().fill('4');
		await memory().fill('511');
		await page.getByLabelText('Memory unit').selectOptions('Mi');
		await expect.element(memory()).toHaveAttribute('aria-invalid', 'true');
		await expect.element(page.getByText('Must be at least 512Mi')).toBeInTheDocument();
		await expect.element(saveButton()).toBeDisabled();
	});

	it('rejects shrinking storage', async () => {
		render(SandboxResourcesForm, {
			project: view({ sandboxSettings: { cpu: null, memory: null, storage: '40Gi' } }),
			save: vi.fn()
		});
		await storage().fill('30');
		await expect.element(storage()).toHaveAttribute('aria-invalid', 'true');
		await expect.element(saveButton()).toBeDisabled();
	});

	it('confirms with the queued copy; cancel writes nothing', async () => {
		const save = vi.fn(async () => {});
		render(SandboxResourcesForm, { project: view(), save });
		await cpu().fill('8');
		await saveButton().click();
		const dialog = page.getByRole('dialog');
		await expect.element(dialog).toHaveTextContent(/queued until running jobs finish/);
		await expect.element(dialog).not.toHaveTextContent(/recreated|stopped/);
		await page.getByRole('button', { name: 'Cancel' }).click();
		expect(save).not.toHaveBeenCalled();
	});

	it('confirm saves, omitting unchanged storage, and shows the checklist', async () => {
		const save = vi.fn(async () => {});
		render(SandboxResourcesForm, {
			project: view({ pendingChange: { generation: 1, since: 'x', reason: 'cpu' } }),
			save
		});
		await cpu().fill('8');
		await memory().fill('16');
		await saveButton().click();
		await page.getByRole('button', { name: 'Save changes' }).click();
		expect(save).toHaveBeenCalledWith({ cpu: 8, memory: '16Gi', storage: undefined });
		await expect
			.element(page.getByRole('list', { name: 'Sandbox change progress' }))
			.toBeInTheDocument();
		await expect.element(page.getByText(/Queued \(waiting for running jobs/)).toBeInTheDocument();
		// The dialog (which carries the confirm copy) closes once the save settles; the queued
		// notice must come from the form itself, not from a dialog that is still unmounting.
		await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
		await expect.element(page.getByText(POD_SHAPE_QUEUED_COPY)).toBeInTheDocument();
	});

	it('sends null for a cleared limit', async () => {
		const save = vi.fn(async () => {});
		render(SandboxResourcesForm, {
			project: view({ sandboxSettings: { cpu: 4, memory: null, storage: null } }),
			save
		});
		await cpu().fill('');
		await saveButton().click();
		await page.getByRole('button', { name: 'Save changes' }).click();
		expect(save).toHaveBeenCalledWith({ cpu: null, memory: null, storage: undefined });
	});

	it('renders a failed step with its message', async () => {
		const save = vi.fn(async () => {});
		render(SandboxResourcesForm, {
			project: view({
				conditions: [
					{
						type: 'SandboxReady',
						status: 'False',
						reason: 'SandboxFailed',
						message: 'no capacity'
					}
				]
			}),
			save
		});
		await cpu().fill('8');
		await saveButton().click();
		await page.getByRole('button', { name: 'Save changes' }).click();
		await expect.element(page.getByText('no capacity')).toBeInTheDocument();
	});

	it('shows storage growth progress without a save', async () => {
		render(SandboxResourcesForm, {
			project: view({
				conditions: [{ type: 'ResourcesReady', status: 'False', reason: 'WorkspaceResizing' }]
			}),
			save: vi.fn()
		});
		await expect.element(page.getByText('Growing workspace storage')).toBeInTheDocument();
	});

	it('shows an error and no checklist when saving fails', async () => {
		const save = vi.fn(async () => {
			throw new Error('boom');
		});
		render(SandboxResourcesForm, { project: view(), save });
		await cpu().fill('8');
		await saveButton().click();
		await page.getByRole('button', { name: 'Save changes' }).click();
		await expect
			.element(page.getByText('Could not save the sandbox resources.'))
			.toBeInTheDocument();
		await expect
			.element(page.getByRole('list', { name: 'Sandbox change progress' }))
			.not.toBeInTheDocument();
	});

	it('is read-only for non-owners', async () => {
		render(SandboxResourcesForm, {
			project: view({ sandboxSettings: { cpu: 8, memory: null, storage: null } }),
			save: vi.fn(),
			canEdit: false
		});
		await expect.element(page.getByText('Only the project owner')).toBeInTheDocument();
		await expect.element(cpu()).not.toBeInTheDocument();
		await expect.element(saveButton()).not.toBeInTheDocument();
	});
});
