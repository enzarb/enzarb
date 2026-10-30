import { expect, test } from '@playwright/test';
import { E2E_PROJECT } from './constants.ts';
import { installFakeSandbox } from './fake-sandbox.ts';
import { E2E_WAIT_RUN_ID } from './stubs/tracks.ts';

// The Tasks pages against the real remote functions; tracks and the GitHub reads are stubs
// (e2e/stubs/tracks.ts, tasks-github.ts). Actions must change the lists without a reload.
const BASE = `/projects/${E2E_PROJECT}/tasks`;

test('lists what waits on a human with links to GitHub and the run', async ({ page }) => {
	await installFakeSandbox(page);
	await page.goto(BASE);
	const wait = page.getByRole('listitem', { name: /Waiting for you/ });
	await expect(wait).toContainText('issue_comment.created');
	await expect(wait.getByRole('link', { name: 'Run' })).toHaveAttribute(
		'href',
		`/projects/${E2E_PROJECT}/runs/${E2E_WAIT_RUN_ID}`
	);
	const review = page.getByRole('listitem', { name: /Pull requests to review/ });
	await expect(review.getByRole('link', { name: 'GitHub' })).toHaveAttribute(
		'href',
		'https://github.com/e2e-org/app/pull/50'
	);
	await expect(page.getByRole('listitem', { name: /Failed runs/ })).toContainText('timeout');
	// The top-level navigation shows the count.
	await expect(page.getByRole('link', { name: /^Tasks, \d+ waiting$/ })).toBeVisible();
});

test('Implement starts the workflow and the issue leaves the list', async ({ page }) => {
	await installFakeSandbox(page);
	await page.goto('/tasks');
	const triaged = page.getByRole('listitem', { name: /Triaged issues: Triaged bug/ });
	await expect(triaged).toContainText('e2e-project · e2e-org/app#9');
	await triaged.getByRole('button', { name: 'Implement' }).click();
	await expect(page.getByText('Started implement-labeled-issues on #9.')).toBeVisible();
	await expect(page.getByRole('listitem', { name: /Triaged issues: Triaged bug/ })).toHaveCount(0);
});

test('Retry restarts a failed run and Stop ends a wait', async ({ page }) => {
	await installFakeSandbox(page);
	await page.goto(BASE);
	await page
		.getByRole('listitem', { name: /Failed runs/ })
		.getByRole('button', { name: 'Retry' })
		.click();
	await expect(page.getByText('Started fix-ci on #12.')).toBeVisible();
	await expect(page.getByRole('listitem', { name: /Failed runs/ })).toHaveCount(0);
	// Stop is idempotent in the stub; the page still answers.
	await page
		.getByRole('listitem', { name: /Waiting for you/ })
		.getByRole('button', { name: 'Stop' })
		.click();
	await expect(page.getByText('Stopping the run.')).toBeVisible();
});
