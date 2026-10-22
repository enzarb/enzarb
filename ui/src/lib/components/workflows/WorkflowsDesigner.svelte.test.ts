import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import WorkflowsDesigner, { type WorkflowsApi } from './WorkflowsDesigner.svelte';
import type { DryRunApi } from './DryRunPanel.svelte';
import {
	exampleWorkflow,
	toWire,
	type DryRunResult,
	type WorkflowsFile
} from '#lib/workflows-types.ts';

const wire = toWire({ workflows: [exampleWorkflow('fix-issue')] });

const file = (over: Partial<WorkflowsFile> = {}): WorkflowsFile => ({
	repo: 'github:acme/w',
	branch: 'main',
	path: '.ez/workflows.toml',
	sha: 'a'.repeat(40),
	text: 'version = 1\n',
	file: wire,
	report: [],
	modesToml: '[modes.triage]\n[modes.implement]\n',
	modeIds: ['triage', 'implement'],
	...over
});

function fakeApi(over: Partial<WorkflowsApi> = {}): WorkflowsApi {
	return {
		render: vi.fn(async () => ({
			ok: true as const,
			text: 'version = 1\n# rendered\n',
			diagnostics: []
		})),
		check: vi.fn(async () => ({ ok: true as const, diagnostics: [], file: wire })),
		save: vi.fn(async () => ({ ok: true as const, unchanged: false, commitUrl: 'https://c' })),
		reload: vi.fn(),
		...over
	};
}

const result: DryRunResult = {
	event_id: 7,
	triggers: [{ workflow: 'fix-issue', matched: true, on: 'issues.labeled', reason: 'label is ez' }],
	traces: [
		{
			workflow: 'fix-issue',
			forced: false,
			steps: [
				{
					node: 'triage',
					node_type: 'agent',
					visit: 1,
					rendered: { type: 'agent', mode: 'triage', instruction: 'Issue #3: T', push: false },
					mode: { id: 'triage', tools: null }
				}
			],
			end: {
				kind: 'needs_outcome',
				node: 'triage',
				node_type: 'agent',
				outputs: ['result', 'git', 'exit'],
				edges: [{ if: "result.decision == 'fix'", to: 'implement' }, { to: 'ask' }]
			}
		}
	]
};

function fakeDryRun(over: Partial<DryRunApi> = {}): DryRunApi {
	return {
		events: vi.fn(async (before?: number) =>
			before
				? []
				: [
						{
							id: 7,
							event: 'issues',
							action: 'labeled',
							repo: 'acme/w',
							sender: 'octo',
							summary: 'Crash on start',
							receivedAt: '2026-10-01T00:00:00Z'
						}
					]
		),
		run: vi.fn(async () => ({
			ok: true as const,
			report: { diagnostics: [] },
			results: [result],
			missing: []
		})),
		...over
	};
}

const commit = () => page.getByRole('button', { name: 'Commit to main' });
const node = (id: string) => page.getByRole('button', { name: new RegExp(`^${id},`) });

describe('WorkflowsDesigner', () => {
	it('draws the graph, edits a node and saves the document', async () => {
		const api = fakeApi();
		const saved = vi.fn();
		render(WorkflowsDesigner, { file: file(), api, dryRun: fakeDryRun(), saved });
		await expect.element(node('triage')).toHaveAccessibleName(/start/);
		await expect.element(commit()).toBeDisabled();

		await node('open_pr').click();
		await page.getByRole('textbox', { name: 'Title' }).fill('Fix it');
		await expect.poll(() => vi.mocked(api.render).mock.calls.length).toBeGreaterThan(0);
		await expect.element(commit()).toBeEnabled();
		await commit().click();

		await expect.poll(() => vi.mocked(api.save).mock.calls.length).toBe(1);
		const input = vi.mocked(api.save).mock.calls[0][0];
		expect(input.file?.workflows?.['fix-issue'].nodes.open_pr).toMatchObject({
			type: 'action',
			title: 'Fix it'
		});
		await expect.poll(() => saved.mock.calls.length).toBe(1);
	});

	it('renames a node together with the edges into it', async () => {
		const api = fakeApi();
		render(WorkflowsDesigner, { file: file(), api, dryRun: fakeDryRun(), saved: vi.fn() });
		await node('ask').click();
		await page.getByRole('textbox', { name: 'Node id' }).fill('ask_more');
		await commit().click();
		await expect.poll(() => vi.mocked(api.save).mock.calls.length).toBe(1);
		const wf = vi.mocked(api.save).mock.calls[0][0].file!.workflows!['fix-issue'];
		expect(wf.nodes).toHaveProperty('ask_more');
		expect(wf.nodes.triage).toMatchObject({ next: [{}, { to: 'ask_more' }] });
	});

	it('lists diagnostics, selects their node and refuses to save with errors', async () => {
		const diagnostics = [
			{
				severity: 'error' as const,
				path: 'workflows.fix-issue.nodes.implement.mode',
				message: 'there is no mode `implement`'
			}
		];
		const api = fakeApi({
			render: vi.fn(async () => ({ ok: true as const, text: 'x', diagnostics }))
		});
		render(WorkflowsDesigner, { file: file(), api, dryRun: fakeDryRun(), saved: vi.fn() });
		await expect.element(node('implement')).toHaveAccessibleName(/has errors/);
		await page.getByRole('button', { name: 'workflows.fix-issue.nodes.implement.mode' }).click();
		await expect.element(page.getByRole('textbox', { name: 'Node id' })).toHaveValue('implement');
		await page.getByRole('textbox', { name: 'Instruction' }).fill('changed');
		await commit().click();
		await expect
			.element(page.getByText(/Fix the errors listed under Validation/))
			.toBeInTheDocument();
		expect(api.save).not.toHaveBeenCalled();
	});

	it('dry-runs chosen events and continues with an assumed outcome', async () => {
		const dryRun = fakeDryRun();
		render(WorkflowsDesigner, { file: file(), api: fakeApi(), dryRun, saved: vi.fn() });
		await page.getByRole('checkbox', { name: /#7 issues\.labeled: Crash on start/ }).click();
		await page.getByRole('button', { name: 'Dry run' }).click();
		await expect.element(page.getByText('label is ez')).toBeInTheDocument();
		expect(vi.mocked(dryRun.run).mock.calls[0][0]).toEqual({
			text: 'version = 1\n# rendered\n',
			eventIds: [7],
			options: {}
		});
		const outputs = page.getByRole('textbox', { name: /Assumed outputs of triage/ });
		await expect.element(outputs).toHaveValue(/"decision": "fix"/);
		await page.getByRole('button', { name: 'Assume and continue' }).click();
		await expect.poll(() => vi.mocked(dryRun.run).mock.calls.length).toBe(2);
		const second = vi.mocked(dryRun.run).mock.calls[1][0];
		expect(second.eventIds).toEqual([7]);
		expect(second.options?.assume?.triage).toMatchObject({ result: { decision: 'fix' } });

		await page.getByRole('button', { name: 'Show on the graph' }).click();
		await expect.element(page.getByRole('button', { name: 'Clear the trace' })).toBeInTheDocument();
	});

	it('starts a missing file from the example', async () => {
		render(WorkflowsDesigner, {
			file: file({ sha: null, text: null, file: { version: 1 } }),
			api: fakeApi(),
			dryRun: fakeDryRun(),
			saved: vi.fn()
		});
		await expect
			.element(page.getByText('There is no .ez/workflows.toml on main yet.', { exact: false }))
			.toBeInTheDocument();
		await page.getByRole('button', { name: 'Start from the example' }).click();
		await expect.element(node('triage')).toBeInTheDocument();
		await expect.element(commit()).toBeEnabled();
	});

	it('opens an unparsable file in the TOML tab and saves the text', async () => {
		const api = fakeApi();
		render(WorkflowsDesigner, {
			file: file({ text: 'bad =', file: null }),
			api,
			dryRun: fakeDryRun(),
			saved: vi.fn()
		});
		await expect
			.element(page.getByText('The file does not parse as workflows'))
			.toBeInTheDocument();
		const editor = page.getByRole('textbox', { name: '.ez/workflows.toml' });
		await expect.element(editor).toHaveValue('bad =');
		await editor.fill('version = 1\n');
		await commit().click();
		await expect
			.poll(() => vi.mocked(api.save).mock.calls[0]?.[0])
			.toEqual({ text: 'version = 1\n', message: '' });
	});
});
