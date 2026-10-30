import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import RulesView from './RulesView.svelte';
import type { DryRunApi } from './DryRunPanel.svelte';
import type { WorkflowsApi } from './WorkflowsDesigner.svelte';
import { exampleWorkflow, toWire, type WorkflowsFile } from '#lib/workflows-types.ts';

const wire = toWire({ workflows: [exampleWorkflow('fix-issue')] });
const file: WorkflowsFile = {
	repo: 'github:acme/w',
	branch: 'main',
	path: '.ez/workflows.toml',
	sha: 'a'.repeat(40),
	text: 'version = 1\n',
	file: wire,
	report: [],
	modesToml: '[modes.triage]\n',
	modeIds: ['triage']
};

const api = (): WorkflowsApi => ({
	render: vi.fn(async () => ({ ok: true as const, text: 'x', diagnostics: [] })),
	check: vi.fn(async () => ({ ok: true as const, diagnostics: [], file: wire })),
	save: vi.fn(async () => ({ ok: true as const, unchanged: false, commitUrl: null })),
	reload: vi.fn()
});
const dry = (): DryRunApi => ({
	events: vi.fn(async (before?: number) =>
		before
			? []
			: [
					{
						id: 7,
						event: 'issues',
						action: 'labeled',
						repo: 'acme/w',
						sender: null,
						summary: null,
						receivedAt: '2026-10-01T00:00:00Z'
					}
				]
	),
	run: vi.fn(async () => ({
		ok: true as const,
		report: { diagnostics: [] },
		results: [
			{
				event_id: 7,
				triggers: [{ workflow: 'fix-issue', matched: true, on: 'issues.labeled', reason: '' }],
				traces: []
			}
		],
		missing: []
	}))
});

describe('RulesView', () => {
	it('shows a rule sentence with modes and recent matches', async () => {
		render(RulesView, { file, api: api(), dryRun: dry(), saved: vi.fn(), advanced: vi.fn() });
		const rule = page.getByRole('listitem', { name: 'Rule fix-issue' });
		await expect.element(rule).toHaveTextContent('When');
		await expect.element(rule).toHaveTextContent('an issue is labeled');
		await expect.element(rule).toHaveTextContent('Using mode');
		await expect.element(rule).toHaveTextContent('triage');
		await expect.element(rule).toHaveTextContent('Would have matched 1 recent event');
	});

	it('edits the condition and saves through the save path', async () => {
		const a = api();
		const saved = vi.fn();
		render(RulesView, { file, api: a, dryRun: dry(), saved, advanced: vi.fn() });
		const commit = page.getByRole('button', { name: 'Commit to main' });
		await expect.element(commit).toBeDisabled();
		await page
			.getByRole('textbox', { name: 'Only if, for fix-issue' })
			.fill("event.label.name == 'urgent'");
		await commit.click();
		await expect.poll(() => vi.mocked(a.save).mock.calls.length).toBe(1);
		expect(vi.mocked(a.save).mock.calls[0][0].file?.workflows?.['fix-issue'].if).toBe(
			"event.label.name == 'urgent'"
		);
		await expect.poll(() => saved.mock.calls.length).toBe(1);
	});

	it('keeps several trigger events, one per line', async () => {
		const a = api();
		render(RulesView, { file, api: a, dryRun: dry(), saved: vi.fn(), advanced: vi.fn() });
		await page
			.getByRole('textbox', { name: 'Events for fix-issue (one per line)' })
			.fill('issues.labeled\npull_request.opened');
		await page.getByRole('button', { name: 'Commit to main' }).click();
		await expect.poll(() => vi.mocked(a.save).mock.calls.length).toBe(1);
		expect(vi.mocked(a.save).mock.calls[0][0].file?.workflows?.['fix-issue'].on).toEqual([
			'issues.labeled',
			'pull_request.opened'
		]);
	});

	it('opens the designer for a workflow', async () => {
		const advanced = vi.fn();
		render(RulesView, { file, api: api(), dryRun: dry(), saved: vi.fn(), advanced });
		await page.getByRole('button', { name: 'Advanced' }).click();
		expect(advanced).toHaveBeenCalledWith('fix-issue');
	});
});
