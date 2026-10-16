import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';
import { EXAMPLE_ISSUE } from '#lib/workflow/example.ts';
import WorkflowEditor from './WorkflowEditor.svelte';

describe('WorkflowEditor SSR', () => {
	it('renders on the server (Svelte Flow does not touch window)', async () => {
		const { body } = await render(WorkflowEditor, {
			props: {
				source: EXAMPLE_ISSUE,
				path: '.enzarb/workflows/issue.yaml',
				loadFile: async () => null,
				propose: async () => ({ url: '' })
			}
		});
		expect(body).toContain('Propose change');
		expect(body).toContain('The definition is valid.');
	});
});
