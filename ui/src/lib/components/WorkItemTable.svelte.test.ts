import { page, userEvent } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Usage } from '#lib/usage.ts';
import type { WorkItemSummary } from '#lib/work-items.ts';
import WorkItemTable from './WorkItemTable.svelte';

const usage = (over: Partial<Usage>): Usage => ({
	inputTokens: null,
	outputTokens: null,
	cacheReadTokens: null,
	cacheWriteTokens: null,
	models: [],
	costUsd: null,
	...over
});

const item = (id: number, title: string, u: Usage | null): WorkItemSummary => ({
	id,
	key: `github:acme/web#${id}`,
	project: 'web',
	workflow: 'issue',
	state: 'Planning',
	title,
	url: null,
	updatedAt: '2026-10-13T12:00:00.000Z',
	usage: u
});

const items = [
	item(
		1,
		'Alpha',
		usage({
			inputTokens: 12_345,
			outputTokens: 1200,
			cacheReadTokens: 9000,
			cacheWriteTokens: 50,
			costUsd: 0.42
		})
	),
	item(2, 'Beta', null),
	item(3, 'Gamma', usage({ inputTokens: 500, outputTokens: 90, costUsd: 3.1 })),
	item(4, 'Delta', usage({ inputTokens: 80_000, outputTokens: 10 }))
];

/** The item titles in row order. */
const order = () =>
	page
		.getByRole('row')
		.elements()
		.slice(1)
		.map((r) => (r.querySelector('a')?.textContent ?? '').trim());

describe('WorkItemTable', () => {
	it('shows tokens in, out and cost, compact, with a dash for unknown', async () => {
		render(WorkItemTable, { items, label: 'Work' });
		const alpha = page.getByRole('row').nth(1);
		await expect.element(alpha).toHaveTextContent('12.3k');
		await expect.element(alpha).toHaveTextContent('1.2k');
		await expect.element(alpha).toHaveTextContent('$0.42');
		const beta = page.getByRole('row').nth(2);
		await expect.element(beta).toHaveTextContent('—');
		await expect.element(beta).not.toHaveTextContent('$0');
		// a cost nobody reported is a dash, not $0.00, and carries no "est." marker
		const delta = page.getByRole('row').nth(4);
		await expect.element(delta).toHaveTextContent('80k');
		await expect.element(delta).not.toHaveTextContent('$');
		await expect.element(page.getByRole('table')).not.toHaveTextContent(/est\./i);
	});

	it('breaks cache tokens down in a tooltip on the In figure', async () => {
		render(WorkItemTable, { items: [items[0]], label: 'Work' });
		await userEvent.hover(page.getByRole('button', { name: '12.3k' }));
		await expect.element(page.getByText('Cache read 9k · Cache write 50')).toBeVisible();
	});

	it('sorts by a usage column, unknown last in both directions', async () => {
		render(WorkItemTable, { items, label: 'Work' });
		expect(order()).toEqual(['Alpha', 'Beta', 'Gamma', 'Delta']); // as given
		const header = page.getByRole('columnheader', { name: /Tokens in/ });
		await header.getByRole('button').click();
		expect(order()).toEqual(['Delta', 'Alpha', 'Gamma', 'Beta']);
		await expect.element(header).toHaveAttribute('aria-sort', 'descending');
		await header.getByRole('button').click();
		expect(order()).toEqual(['Gamma', 'Alpha', 'Delta', 'Beta']);
		await expect.element(header).toHaveAttribute('aria-sort', 'ascending');
		await page.getByRole('columnheader', { name: /Cost/ }).getByRole('button').click();
		expect(order()).toEqual(['Gamma', 'Alpha', 'Beta', 'Delta']);
	});

	it('leaves the other columns unsortable', async () => {
		render(WorkItemTable, { items, label: 'Work' });
		await expect
			.element(page.getByRole('columnheader', { name: 'Project' }))
			.not.toHaveAttribute('aria-sort');
	});
});
