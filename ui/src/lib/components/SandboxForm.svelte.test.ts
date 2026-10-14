import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import SandboxForm from './SandboxForm.svelte';
import { POD_SHAPE_PRESAVE_COPY } from '#lib/pending-change.ts';
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

const props = () => ({ saveResources: vi.fn(async () => {}), saveMaxRuns: vi.fn(async () => {}) });

const cpu = () => page.getByRole('textbox', { name: 'CPU (cores)' });
const memory = () => page.getByRole('textbox', { name: 'Memory' });
const storage = () => page.getByRole('textbox', { name: 'Storage' });
const maxRuns = () => page.getByRole('spinbutton', { name: 'Concurrent runs' });
const editButton = () => page.getByRole('button', { name: 'Edit sandbox' });
const saveButton = () => page.getByRole('button', { name: 'Save sandbox' });
const notice = () => page.getByText(POD_SHAPE_PRESAVE_COPY);

async function edit(p: ProjectView = view(), handlers = props()) {
	render(SandboxForm, { project: p, ...handlers });
	await editButton().click();
	return handlers;
}

describe('SandboxForm', () => {
	it('is read-only with an Edit button, showing defaults and concurrency', async () => {
		render(SandboxForm, { project: view(), ...props() });
		await expect.element(page.getByText('not set (no limit)').first()).toBeInTheDocument();
		await expect.element(page.getByText('20Gi (default)')).toBeInTheDocument();
		await expect.element(page.getByText('Concurrent runs')).toBeInTheDocument();
		await expect.element(editButton()).toBeInTheDocument();
		await expect.element(cpu()).not.toBeInTheDocument();
		await expect.element(notice()).not.toBeInTheDocument();
	});

	it('shows the current values as the initial text', async () => {
		await edit(view({ sandboxSettings: { cpu: 8, memory: '16Gi', storage: '40Gi' }, maxRuns: 4 }));
		await expect.element(cpu()).toHaveValue('8');
		await expect.element(memory()).toHaveValue('16Gi');
		await expect.element(storage()).toHaveValue('40Gi');
		await expect.element(maxRuns()).toHaveValue(4);
		await expect.element(saveButton()).toBeDisabled();
	});

	it('flags invalid values inline while typing and keeps Save disabled', async () => {
		await edit();
		await cpu().fill('500m');
		await expect.element(cpu()).toHaveAttribute('aria-invalid', 'true');
		await expect.element(page.getByText(/Fractions of a core/)).toBeInTheDocument();
		await cpu().fill('4');
		await memory().fill('511Mi');
		await expect.element(page.getByText('Must be at least 512Mi')).toBeInTheDocument();
		await memory().fill('8GB');
		await expect.element(memory()).toHaveAttribute('aria-invalid', 'true');
		await expect.element(saveButton()).toBeDisabled();
	});

	it('rejects shrinking storage', async () => {
		await edit(view({ sandboxSettings: { cpu: null, memory: null, storage: '40Gi' } }));
		await storage().fill('30Gi');
		await expect.element(storage()).toHaveAttribute('aria-invalid', 'true');
		await expect.element(saveButton()).toBeDisabled();
	});

	it('shows the notice only while an edit is unsaved, and not after reverting', async () => {
		await edit();
		await expect.element(notice()).not.toBeInTheDocument();
		await cpu().fill('8');
		await expect.element(notice()).toBeInTheDocument();
		await cpu().fill('');
		await expect.element(notice()).not.toBeInTheDocument();
	});

	it('shows the notice for an invalid edit and a max-runs edit', async () => {
		await edit();
		await cpu().fill('abc');
		await expect.element(notice()).toBeInTheDocument();
		await cpu().fill('');
		await maxRuns().fill('5');
		await expect.element(notice()).toBeInTheDocument();
	});

	it('hides the notice after Save and after Cancel', async () => {
		const h = await edit();
		await cpu().fill('8');
		await saveButton().click();
		expect(h.saveResources).toHaveBeenCalled();
		await expect.element(notice()).not.toBeInTheDocument();
		await editButton().click();
		await cpu().fill('9');
		await expect.element(notice()).toBeInTheDocument();
		await page.getByRole('button', { name: 'Cancel' }).click();
		await expect.element(notice()).not.toBeInTheDocument();
		await editButton().click();
		await expect.element(cpu()).toHaveValue('');
	});

	it('saves the normalised string, omitting unchanged storage, and shows the checklist', async () => {
		const h = await edit(view({ pendingChange: { generation: 1, since: 'x', reason: 'cpu' } }));
		await cpu().fill('8');
		await memory().fill('16384Mi');
		await saveButton().click();
		expect(h.saveResources).toHaveBeenCalledWith({ cpu: 8, memory: '16Gi', storage: undefined });
		expect(h.saveMaxRuns).not.toHaveBeenCalled();
		await expect
			.element(page.getByRole('list', { name: 'Sandbox change progress' }))
			.toBeInTheDocument();
		await expect.element(editButton()).toBeInTheDocument();
	});

	it('sends changed storage and null for a cleared limit', async () => {
		const h = await edit(view({ sandboxSettings: { cpu: 4, memory: null, storage: null } }));
		await cpu().fill('');
		await storage().fill('2Ti');
		await saveButton().click();
		expect(h.saveResources).toHaveBeenCalledWith({ cpu: null, memory: null, storage: '2048Gi' });
	});

	it('one Save runs both commands, each only for what changed', async () => {
		const h = await edit();
		await cpu().fill('2');
		await maxRuns().fill('5');
		await saveButton().click();
		expect(h.saveMaxRuns).toHaveBeenCalledWith(5);
		expect(h.saveResources).toHaveBeenCalledTimes(1);
	});

	it('a max-runs-only save skips the resources command and sends null for the default', async () => {
		const h = await edit(view({ maxRuns: 5 }));
		await maxRuns().fill('2');
		await saveButton().click();
		expect(h.saveMaxRuns).toHaveBeenCalledWith(null);
		expect(h.saveResources).not.toHaveBeenCalled();
	});

	it('reports a partial failure and a retry sends only the failed part', async () => {
		const h = props();
		h.saveResources.mockRejectedValueOnce(new Error('boom'));
		await edit(view(), h);
		await cpu().fill('8');
		await maxRuns().fill('5');
		await saveButton().click();
		await expect
			.element(page.getByText('Concurrency saved, but the sandbox resources could not be saved.'))
			.toBeInTheDocument();
		await expect
			.element(page.getByRole('list', { name: 'Sandbox change progress' }))
			.not.toBeInTheDocument();
		await saveButton().click();
		expect(h.saveMaxRuns).toHaveBeenCalledTimes(2); // the fake project does not refresh
	});

	it('shows an error and no checklist when saving resources fails', async () => {
		const h = props();
		h.saveResources.mockRejectedValueOnce(new Error('boom'));
		await edit(view(), h);
		await cpu().fill('8');
		await saveButton().click();
		await expect
			.element(page.getByText('Could not save the sandbox resources.'))
			.toBeInTheDocument();
	});

	it('renders a failed step with its message', async () => {
		await edit(
			view({
				conditions: [
					{ type: 'SandboxReady', status: 'False', reason: 'SandboxFailed', message: 'no capacity' }
				]
			})
		);
		await cpu().fill('8');
		await saveButton().click();
		await expect.element(page.getByText('no capacity')).toBeInTheDocument();
	});

	it('shows storage growth progress without a save', async () => {
		render(SandboxForm, {
			project: view({
				conditions: [{ type: 'ResourcesReady', status: 'False', reason: 'WorkspaceResizing' }]
			}),
			...props()
		});
		await expect.element(page.getByText('Growing workspace storage')).toBeInTheDocument();
	});

	it('is read-only for non-owners', async () => {
		render(SandboxForm, {
			project: view({ sandboxSettings: { cpu: 8, memory: null, storage: null } }),
			...props(),
			canEdit: false
		});
		await expect.element(page.getByText('Only the project owner')).toBeInTheDocument();
		await expect.element(editButton()).not.toBeInTheDocument();
	});
});
