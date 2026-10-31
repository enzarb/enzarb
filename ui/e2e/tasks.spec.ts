import { expect, test } from '@playwright/test';
import { E2E_PROJECT } from './constants.ts';
import { installFakeSandbox } from './fake-sandbox.ts';
import { E2E_FAILED_RUN_ID, E2E_WAIT_RUN_ID } from './stubs/tracks.ts';

// The Tasks pages against the real remote functions; tracks and the GitHub reads are stubs
// (e2e/stubs/tracks.ts, tasks-github.ts). Master-detail: the list selects through `?task=`, the
// default action is the primary button, and actions change the lists without a reload.
const BASE = `/projects/${E2E_PROJECT}/tasks`;

test('lists what waits on a human; selecting shows readable links and the run', async ({
	page
}) => {
	await installFakeSandbox(page);
	await page.goto(BASE);
	await expect(page.getByRole('listitem', { name: /Waiting for you/ })).toContainText('waiting');
	await expect(page.getByRole('listitem', { name: /Failed runs/ })).toContainText('timeout');

	await page
		.getByRole('listitem', { name: /Waiting for you/ })
		.getByRole('link')
		.click();
	await expect(page).toHaveURL(new RegExp(`task=run%3A${E2E_WAIT_RUN_ID}`));
	const detail = page.getByRole('article', { name: /^Task:/ });
	await expect(detail.getByRole('link', { name: 'app#11' })).toHaveAttribute(
		'href',
		/github\.com\/e2e-org\/app\/issues\/11/
	);
	await expect(detail.getByRole('link', { name: 'ask-first #44444444' })).toHaveAttribute(
		'href',
		`/projects/${E2E_PROJECT}/runs/${E2E_WAIT_RUN_ID}`
	);
	await expect(detail.getByRole('button', { name: 'Stop for now' })).toBeVisible();
	await expect(page.getByPlaceholder(/coming next/)).toBeDisabled();
	await expect(page.getByRole('link', { name: 'GitHub', exact: true })).toHaveCount(0);

	// The selection survives a reload.
	await page.reload();
	await expect(page.getByRole('article', { name: /^Task:/ })).toBeVisible();

	await page
		.getByRole('listitem', { name: /Pull requests to review/ })
		.getByRole('link')
		.click();
	await expect(page.getByRole('link', { name: 'app#50', exact: true }).first()).toHaveAttribute(
		'href',
		'https://github.com/e2e-org/app/pull/50'
	);
	await expect(page.getByRole('link', { name: 'Open diff' })).toHaveAttribute(
		'href',
		'https://github.com/e2e-org/app/pull/50/files'
	);
	// The top-level navigation shows the count.
	await expect(page.getByRole('link', { name: /^Tasks, \d+ waiting$/ })).toBeVisible();
});

test('Implement starts the workflow and the issue leaves the list', async ({ page }) => {
	await installFakeSandbox(page);
	await page.goto('/tasks');
	const triaged = page.getByRole('listitem', { name: /Triaged issues: Triaged bug/ });
	await expect(triaged).toContainText('e2e-project');
	await triaged.getByRole('link').click();
	await expect(page.getByText('The app crashes on start.')).toBeVisible();
	await page.getByRole('button', { name: 'Implement' }).click();
	await expect(page.getByText('Started implement-labeled-issues on #9.')).toBeVisible();
	// The task left the list, but the detail keeps the confirmation with the new run's link.
	const done = page.getByTestId('task-done');
	await expect(done).toContainText('Started implement-labeled-issues on #9.');
	await expect(done.getByRole('link', { name: /^implement-labeled-issues #/ })).toBeVisible();
	await expect(page.getByRole('listitem', { name: /Triaged issues: Triaged bug/ })).toHaveCount(0);
});

test('Retry restarts a failed run and Stop for now ends a wait', async ({ page }) => {
	await installFakeSandbox(page);
	await page.goto(`${BASE}?task=${encodeURIComponent(`run:${E2E_FAILED_RUN_ID}`)}`);
	await page.getByRole('button', { name: 'Retry' }).click();
	await expect(page.getByText('Started fix-ci on #12.')).toBeVisible();
	await expect(page.getByRole('listitem', { name: /Failed runs/ })).toHaveCount(0);
	// Stop is idempotent in the stub; the page still answers.
	await page
		.getByRole('listitem', { name: /Waiting for you/ })
		.getByRole('link')
		.click();
	await page.getByRole('button', { name: 'Stop for now' }).click();
	await expect(page.getByText('Stopping the run.')).toBeVisible();
});

for (const size of [
	{ width: 1280, height: 800 },
	{ width: 1280, height: 1080 },
	{ width: 390, height: 800 }
]) {
	test(`the chat bar stays in view at ${size.width}x${size.height}`, async ({ page }) => {
		await page.setViewportSize(size);
		await installFakeSandbox(page);
		await page.goto(BASE);
		await page
			.getByRole('listitem', { name: /Pull requests to review/ })
			.getByRole('link')
			.click();
		for (const el of [
			page.getByPlaceholder(/coming next/),
			page.getByRole('button', { name: 'Send' })
		]) {
			await expect(el).toBeVisible();
			await expect(el).toBeInViewport({ ratio: 1 });
		}
	});
}

test('on a phone the list and the detail stack, with a back control', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 800 });
	await installFakeSandbox(page);
	await page.goto(BASE);
	await expect(page.getByRole('navigation', { name: 'Tasks' })).toBeVisible();
	await page
		.getByRole('listitem', { name: /Pull requests to review/ })
		.getByRole('link')
		.click();
	await expect(page.getByRole('navigation', { name: 'Tasks' })).toBeHidden();
	await expect(page.getByRole('article', { name: /^Task:/ })).toBeVisible();
	await page.getByRole('link', { name: 'All tasks' }).click();
	await expect(page.getByRole('navigation', { name: 'Tasks' })).toBeVisible();
});
