import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import TableHarness from './__fixtures__/TableHarness.svelte';

describe('DataTable', () => {
	it('renders rows with labelled cells and row actions', async () => {
		render(TableHarness, {
			items: [
				{ id: 1, name: 'Alpha' },
				{ id: 2, name: 'Beta' }
			]
		});
		await expect.element(page.getByRole('table', { name: 'Things' })).toBeVisible();
		await expect
			.element(page.getByRole('cell', { name: 'Alpha', exact: true }))
			.toHaveAttribute('data-label', 'Name');
		await expect.element(page.getByRole('button', { name: 'Open Beta' })).toBeVisible();
	});

	it('keeps explicit table semantics for the collapsed phone layout', async () => {
		render(TableHarness, { items: [{ id: 1, name: 'Alpha' }] });
		await expect.element(page.getByRole('table', { name: 'Things' })).toBeVisible();
		await expect.element(page.getByRole('columnheader', { name: 'Name' })).toBeInTheDocument();
		await expect.element(page.getByRole('row').nth(1)).toBeVisible();
		expect(page.getByRole('rowgroup').elements().length).toBe(2);
		// the row-actions cell keeps its role too
		await expect.element(page.getByRole('cell', { name: 'Open Alpha' })).toBeVisible();
	});

	it('shows the empty state instead of an empty table', async () => {
		render(TableHarness, { items: [] });
		await expect.element(page.getByRole('heading', { name: 'No things', level: 2 })).toBeVisible();
		await expect.element(page.getByRole('table')).not.toBeInTheDocument();
	});

	it('lets the page pick the empty-state heading level', async () => {
		render(TableHarness, { items: [], emptyLevel: 3 });
		await expect.element(page.getByRole('heading', { name: 'No things', level: 3 })).toBeVisible();
	});

	it('shows a labelled skeleton while loading', async () => {
		render(TableHarness, { items: [], loading: true });
		await expect.element(page.getByRole('status')).toHaveTextContent('Loading');
		await expect.element(page.getByRole('table')).not.toBeInTheDocument();
	});
});
