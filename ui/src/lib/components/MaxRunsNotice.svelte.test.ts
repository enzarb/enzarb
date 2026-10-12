import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import MaxRunsNotice from './MaxRunsNotice.svelte';

describe('MaxRunsNotice', () => {
	it('says the change is queued and running jobs are not interrupted', async () => {
		render(MaxRunsNotice, {});
		const notice = page.getByRole('status');
		await expect.element(notice).toHaveTextContent(/queued/);
		await expect.element(notice).toHaveTextContent(/once currently running jobs finish/);
		await expect.element(notice).toHaveTextContent(/not interrupted/);
		await expect.element(notice).not.toHaveTextContent(/may be interrupted/);
		await expect.element(notice).not.toHaveTextContent(/restarts/);
	});
});
