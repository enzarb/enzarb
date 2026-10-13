import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { createRawSnippet } from 'svelte';
import Notice from './Notice.svelte';

const text = (t: string) => createRawSnippet(() => ({ render: () => `<span>${t}</span>` }));

describe('Notice', () => {
	it('is a polite status for info and an alert for errors', async () => {
		const info = render(Notice, { tone: 'info', children: text('FYI') });
		await expect.element(page.getByRole('status')).toHaveTextContent('FYI');
		info.unmount();
		render(Notice, { tone: 'error', title: 'Failed', children: text('Broke') });
		await expect.element(page.getByRole('alert')).toHaveTextContent('Failed');
	});
});
