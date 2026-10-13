import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import JobTable from './JobTable.svelte';

describe('JobTable', () => {
	it('titles its empty state one level below the Jobs section heading', async () => {
		render(JobTable, { jobs: [] });
		await expect
			.element(page.getByRole('heading', { name: 'No jobs yet', level: 3 }))
			.toBeVisible();
	});
});
