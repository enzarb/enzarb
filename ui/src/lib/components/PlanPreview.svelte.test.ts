import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import PlanPreview from './PlanPreview.svelte';

describe('PlanPreview', () => {
	it('is collapsed and loads nothing until opened, then renders the html once', async () => {
		const load = vi.fn(async () => '<h2>The plan</h2>');
		render(PlanPreview, { load });
		expect(load).not.toHaveBeenCalled();
		await page.getByText('Plan', { exact: true }).click();
		await expect.element(page.getByRole('heading', { name: 'The plan' })).toBeVisible();
		await page.getByText('Plan', { exact: true }).click();
		await page.getByText('Plan', { exact: true }).click();
		expect(load).toHaveBeenCalledTimes(1);
	});

	it('reports a failed load', async () => {
		render(PlanPreview, {
			load: async () => {
				throw new Error('x');
			}
		});
		await page.getByText('Plan', { exact: true }).click();
		await expect.element(page.getByText('Could not load the plan.')).toBeVisible();
	});
});
