import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { EXAMPLE_ISSUE } from '#lib/workflow/example.ts';
import WorkflowEditor from './WorkflowEditor.svelte';

const yamlBox = () => page.getByLabelText('Definition (YAML)');

function setup(source = EXAMPLE_ISSUE, path: string | null = '.enzarb/workflows/issue.yaml') {
	const propose = vi.fn(
		async (
			_files: unknown,
			_wf: string,
			step: (id: 'branch' | 'commit' | 'pr', m?: string) => void
		) => {
			step('branch', 'enzarb/workflow-issue-abc123');
			step('commit', 'abc1234 · 1 file');
			step('pr', '#7');
			return { url: 'https://github.com/o/r/pull/7' };
		}
	);
	render(WorkflowEditor, {
		source,
		path,
		loadFile: async () => null,
		propose
	});
	return { propose };
}

describe('WorkflowEditor', () => {
	it('draws every node on the canvas and reports a valid definition', async () => {
		setup();
		for (const id of ['prepare', 'implement', 'open_pr', 'failed', 'done'])
			await expect.element(page.getByTestId(`wf-node-${id}`)).toBeInTheDocument();
		await expect.element(page.getByText('The definition is valid.')).toBeInTheDocument();
		await expect.element(page.getByRole('button', { name: 'Propose change' })).toBeDisabled();
	});

	it('round-trips through the YAML view, keeping comments', async () => {
		setup();
		await page.getByRole('button', { name: 'YAML' }).click();
		await expect.element(yamlBox()).toHaveValue(EXAMPLE_ISSUE);
		await yamlBox().fill(EXAMPLE_ISSUE.replace('mise run prepare', 'mise run setup'));
		await page.getByRole('button', { name: 'Graph' }).click();
		await page.getByTestId('wf-node-prepare').click();
		await expect.element(page.getByLabelText('Run')).toHaveValue('mise run setup');
		await page.getByRole('button', { name: 'YAML' }).click();
		await expect
			.poll(() => (yamlBox().element() as HTMLTextAreaElement).value)
			.toContain('# triggers that create a task');
	});

	it('shows parse errors and validation problems live', async () => {
		setup();
		await page.getByRole('button', { name: 'YAML' }).click();
		await yamlBox().fill('version: [');
		await expect.element(page.getByText('The YAML does not parse')).toBeInTheDocument();
		await expect.element(page.getByRole('button', { name: 'Graph' })).toBeDisabled();
		await yamlBox().fill(EXAMPLE_ISSUE.replace('next: done', 'next: ghost'));
		await expect.element(page.getByText('2 problems')).toBeInTheDocument();
		await expect
			.element(page.getByRole('button', { name: 'node `open_pr`: edge to unknown node `ghost`' }))
			.toBeInTheDocument();
		await expect.element(page.getByRole('button', { name: 'Propose change' })).toBeDisabled();
	});

	it('proposes a change with a step checklist and links the pull request', async () => {
		const { propose } = setup();
		await page.getByTestId('wf-node-prepare').click();
		await page.getByLabelText('Run').fill('mise run setup');
		await page.getByRole('button', { name: 'Propose change' }).click();
		await expect
			.element(page.getByRole('link', { name: 'View the pull request' }))
			.toHaveAttribute('href', 'https://github.com/o/r/pull/7');
		const [files, workflow] = propose.mock.calls[0];
		expect(workflow).toBe('issue');
		expect(files).toEqual([
			{
				path: '.enzarb/workflows/issue.yaml',
				text: expect.stringContaining('mise run setup')
			}
		]);
		await expect
			.element(page.getByRole('list', { name: 'Proposal progress' }))
			.toHaveTextContent(/Create branch.*Commit changes.*Open pull request/);
	});

	it('marks the failing step when a proposal fails', async () => {
		render(WorkflowEditor, {
			source: EXAMPLE_ISSUE,
			path: null,
			loadFile: async () => null,
			propose: async (_f, _w, step) => {
				step('branch');
				throw Object.assign(new Error('x'), { body: { message: 'You need write access' } });
			}
		});
		await page.getByRole('button', { name: 'Propose change' }).click();
		await expect
			.element(page.getByRole('list', { name: 'Proposal progress' }))
			.toHaveTextContent('You need write access');
	});

	it('adds and deletes nodes', async () => {
		setup();
		await page.getByLabelText('Add node').selectOptions('wait');
		await expect.element(page.getByTestId('wf-node-wait')).toBeInTheDocument();
		await expect
			.element(page.getByRole('button', { name: 'node `wait`: `next` is missing' }))
			.toBeInTheDocument();
		await page.getByRole('button', { name: 'Delete node' }).click();
		await expect.element(page.getByTestId('wf-node-wait')).not.toBeInTheDocument();
	});
});
