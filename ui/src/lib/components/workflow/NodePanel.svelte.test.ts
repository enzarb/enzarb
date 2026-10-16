import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { WorkflowNode } from '#lib/workflow/definition.ts';
import NodePanel from './NodePanel.svelte';

function setup(node: WorkflowNode, over: Record<string, unknown> = {}) {
	const n = $state(node);
	const props = {
		node: n,
		ids: ['a', 'b', 'e'],
		isStart: false,
		problems: [] as string[],
		onrename: vi.fn(),
		onretype: vi.fn(),
		onsetstart: vi.fn(),
		ondelete: vi.fn(),
		fileText: vi.fn(() => null as string | null | undefined),
		setFileText: vi.fn(),
		...over
	};
	render(NodePanel, props);
	return { n, props };
}

describe('NodePanel', () => {
	it('edits a command node in place', async () => {
		const { n } = setup({ id: 'a', type: 'command', run: 'x', next: { kind: 'to', to: 'e' } });
		await page.getByLabelText('Run').fill('mise run test');
		expect(n.run).toBe('mise run test');
		await page.getByLabelText('Max visits').fill('3');
		expect(n.max_visits).toBe(3);
		await page.getByLabelText('On limit').selectOptions('b');
		expect(n.on_limit).toBe('b');
		await page.getByLabelText(/Optional/).click();
		expect(n.optional).toBe(true);
	});

	it('reports renames, retypes, start and delete through callbacks', async () => {
		const { props } = setup({ id: 'a', type: 'command', run: 'x' });
		await page.getByLabelText('Id').fill('build');
		page
			.getByLabelText('Id')
			.element()
			.dispatchEvent(new Event('change', { bubbles: true }));
		expect(props.onrename).toHaveBeenCalledWith('build');
		await page.getByLabelText('Type').selectOptions('wait');
		expect(props.onretype).toHaveBeenCalledWith('wait');
		await page.getByRole('button', { name: 'Set as start' }).click();
		expect(props.onsetstart).toHaveBeenCalled();
		await page.getByRole('button', { name: 'Delete node' }).click();
		expect(props.ondelete).toHaveBeenCalled();
	});

	it('shows the node problems inline', async () => {
		setup({ id: 'a', type: 'command', run: '' }, { problems: ['node `a`: `run` is empty'] });
		await expect
			.element(page.getByRole('list', { name: 'Problems of this node' }))
			.toHaveTextContent('`run` is empty');
	});

	it('edits ordered conditions: add before else, reorder, remove', async () => {
		const { n } = setup({
			id: 'a',
			type: 'agent',
			prompt: 'implement',
			next: {
				kind: 'cond',
				conds: [
					{ when: 'pushed', to: 'b' },
					{ when: 'else', to: 'e' }
				]
			}
		});
		await page.getByRole('button', { name: 'Add condition' }).click();
		await page.getByLabelText('Condition 2', { exact: true }).fill('exit_code == 0');
		await page.getByRole('button', { name: 'Move condition 2 up' }).click();
		expect(n.next).toEqual({
			kind: 'cond',
			conds: [
				{ when: 'exit_code == 0', to: 'a' },
				{ when: 'pushed', to: 'b' },
				{ when: 'else', to: 'e' }
			]
		});
		await expect
			.element(page.getByRole('button', { name: 'Move condition 2 down' }))
			.toBeDisabled();
		await page.getByRole('button', { name: 'Remove condition 1' }).click();
		expect(n.next?.kind === 'cond' && n.next.conds.map((c) => c.when)).toEqual(['pushed', 'else']);
		await page.getByLabelText('Condition 1', { exact: true }).fill('pushed ==');
		await expect.element(page.getByRole('alert')).toHaveTextContent('condition `pushed ==`');
	});

	it('shows a built-in prompt read-only and copies it into the repo as a file', async () => {
		const { n, props } = setup({
			id: 'a',
			type: 'agent',
			prompt: 'implement',
			next: { kind: 'to', to: 'e' }
		});
		await expect.element(page.getByLabelText('Built-in template text')).toHaveTextContent(/\S/);
		await page.getByRole('button', { name: 'Copy into repo as file' }).click();
		expect(n.prompt).toEqual({ file: '.enzarb/prompts/implement.md' });
		expect(props.setFileText).toHaveBeenCalledWith(
			'.enzarb/prompts/implement.md',
			expect.stringMatching(/\S/)
		);
	});

	it('edits a prompt file', async () => {
		const { props } = setup(
			{
				id: 'a',
				type: 'agent',
				prompt: { file: '.enzarb/prompts/fix.md' },
				next: { kind: 'to', to: 'e' }
			},
			{ fileText: () => 'Old text' }
		);
		const box = page.getByLabelText('Contents of .enzarb/prompts/fix.md');
		await expect.element(box).toHaveValue('Old text');
		await box.fill('New text');
		expect(props.setFileText).toHaveBeenLastCalledWith('.enzarb/prompts/fix.md', 'New text');
	});
});
