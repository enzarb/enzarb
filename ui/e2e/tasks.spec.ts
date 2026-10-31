import { expect, test } from '@playwright/test';
import { AGENT_REPLY, E2E_PROJECT } from './constants.ts';
import { installFakeSandbox } from './fake-sandbox.ts';
import {
	E2E_FAILED_RUN_ID,
	E2E_PLAN_REVISE_RUN_ID,
	E2E_PLAN_RUN_ID,
	E2E_WAIT_RUN_ID
} from './stubs/tracks.ts';

// The Tasks pages against the real remote functions; tracks and the GitHub reads are stubs
// (e2e/stubs/tracks.ts, tasks-github.ts). Master-detail: the list selects through `?task=`, the
// default action is the primary button, and actions change the lists without a reload.
const BASE = `/projects/${E2E_PROJECT}/tasks`;

test('lists what waits on a human; selecting shows readable links and the run', async ({
	page
}) => {
	await installFakeSandbox(page);
	await page.goto(BASE);
	await expect(page.getByRole('listitem', { name: /Waiting for you: ask-first/ })).toContainText(
		'waiting'
	);
	await expect(page.getByRole('listitem', { name: /Failed runs/ })).toContainText('timeout');

	await page
		.getByRole('listitem', { name: /Waiting for you: ask-first/ })
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
	await expect(page.getByPlaceholder(/Message the agent/)).toBeEnabled();
	await expect(page.getByRole('link', { name: 'GitHub', exact: true })).toHaveCount(0);

	// The selection survives a reload.
	await page.reload();
	await expect(page.getByRole('article', { name: /^Task:/ })).toBeVisible();

	await page
		.getByRole('listitem', { name: /Pull requests to review: Fix #7/ })
		.getByRole('link')
		.click();
	await expect(page.getByRole('link', { name: 'app#50', exact: true }).first()).toHaveAttribute(
		'href',
		'https://github.com/e2e-org/app/pull/50'
	);
	// The run that opened it reads like any run link.
	await expect(page.getByRole('link', { name: 'fix-issue #11111111' })).toHaveAttribute(
		'href',
		`/projects/${E2E_PROJECT}/runs/11111111-1111-4111-8111-111111111111`
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
		.getByRole('listitem', { name: /Waiting for you: ask-first/ })
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
			.getByRole('listitem', { name: /Pull requests to review: Fix #7/ })
			.getByRole('link')
			.click();
		for (const el of [
			page.getByPlaceholder(/Message the agent/),
			page.getByRole('button', { name: 'Send' })
		]) {
			await expect(el).toBeVisible();
			await expect(el).toBeInViewport({ ratio: 1 });
		}
		if (size.width >= 1280) {
			// The detail fills the viewport: the chat bar sits right above the footer, and the
			// page itself does not scroll.
			const gap = await page.evaluate(() => {
				const bar = document.querySelector('.ez-tasks__chat')!.getBoundingClientRect();
				const footer = document.querySelector('.ez-footer')!.getBoundingClientRect();
				return {
					gap: footer.top - bar.bottom,
					scrolls: document.documentElement.scrollHeight > window.innerHeight
				};
			});
			expect(gap.gap).toBeGreaterThanOrEqual(0);
			expect(gap.gap).toBeLessThan(24);
			expect(gap.scrolls).toBe(false);
		}
	});
}

test('on a phone the list and the detail stack, with a back control', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 800 });
	await installFakeSandbox(page);
	await page.goto(BASE);
	await expect(page.getByRole('navigation', { name: 'Tasks' })).toBeVisible();
	await page
		.getByRole('listitem', { name: /Pull requests to review: Fix #7/ })
		.getByRole('link')
		.click();
	await expect(page.getByRole('navigation', { name: 'Tasks' })).toBeHidden();
	await expect(page.getByRole('article', { name: /^Task:/ })).toBeVisible();
	await page.getByRole('link', { name: 'All tasks' }).click();
	await expect(page.getByRole('navigation', { name: 'Tasks' })).toBeVisible();
});

test('the chat bar chats on the task branch and carries the task context', async ({ page }) => {
	const opens: { repo?: string | null; checkout?: unknown }[] = [];
	await installFakeSandbox(page, { onOpen: (o) => opens.push(o) });
	await page.goto(BASE);
	await page
		.getByRole('listitem', { name: /Pull requests to review: Fix #7/ })
		.getByRole('link')
		.click();
	const box = page.getByPlaceholder(/Message the agent/);
	await expect(box).toBeEnabled();
	expect(opens[0].repo).toBe('https://github.com/e2e-org/app.git');
	expect(opens[0].checkout).toMatchObject({ branch: expect.stringMatching(/^ez\//) });
	await box.fill('Does this handle retries?');
	await page.getByRole('button', { name: 'Send' }).click();
	const log = page.getByRole('log', { name: 'Conversation' });
	await expect(log).toContainText('Does this handle retries?');
	await expect(log).toContainText(AGENT_REPLY);
	// The conversation opened above the input, which stays in view.
	await expect(box).toBeInViewport({ ratio: 1 });
	await expect(
		page.getByText('Changes stay in the sandbox until a run pushes them.')
	).toBeVisible();

	// Another task, another chat on its own branch.
	await page.goto(`${BASE}?task=${encodeURIComponent(`run:${E2E_WAIT_RUN_ID}`)}`);
	await expect(page.getByPlaceholder(/Message the agent/)).toBeEnabled();
	const second = (opens.at(-1)?.checkout as { branch: string }).branch;
	expect(second).toMatch(/^ez\//);
	expect(second).not.toBe((opens[0].checkout as { branch: string }).branch);
});

test('a sandbox without chat_checkout asks for an update', async ({ page }) => {
	await installFakeSandbox(page, { chatCheckout: false });
	await page.goto(BASE);
	await page
		.getByRole('listitem', { name: /Pull requests to review: Fix #7/ })
		.getByRole('link')
		.click();
	await expect(page.getByText('Needs an update', { exact: true })).toBeVisible();
});

const planUrl = (id: string) => `${BASE}?task=${encodeURIComponent(`run:${id}`)}`;

test('a plan awaiting approval is shown rendered and Approve starts the implementation', async ({
	page
}) => {
	await page.setViewportSize({ width: 1280, height: 800 });
	await installFakeSandbox(page);
	await page.goto(planUrl(E2E_PLAN_RUN_ID));
	const plan = page.getByTestId('plan-body');
	await expect(plan.getByRole('heading', { name: 'Plan' })).toBeVisible();
	await expect(plan.locator('strong')).toHaveText('regression test');
	// Sanitised: the script tag from the agent's text is not rendered.
	await expect(plan.locator('script')).toHaveCount(0);
	if (process.env.E2E_SHOTS) await page.screenshot({ path: `${process.env.E2E_SHOTS}/plan.png` });
	await page.getByRole('button', { name: 'Approve', exact: true }).click();
	const done = page.getByTestId('task-done');
	await expect(done).toContainText('Plan approved: implementation started.');
	await expect(
		done.getByRole('link', { name: /^implement-labeled-issues #66666666/ })
	).toBeVisible();
});

test('Revise sends the text; a run that already moved on says so', async ({ page }) => {
	await installFakeSandbox(page);
	await page.goto(planUrl(E2E_PLAN_REVISE_RUN_ID));
	await page.getByRole('button', { name: 'Revise' }).click();
	await page.getByLabel('What should change in the plan?').fill('Also cover the Windows path');
	await page.getByRole('button', { name: 'Send revision' }).click();
	await expect(page.getByTestId('task-done')).toContainText('Revision sent');
});

test('the run page shows the same plan controls', async ({ page }) => {
	await installFakeSandbox(page);
	await page.goto(`/projects/${E2E_PROJECT}/runs/${E2E_PLAN_REVISE_RUN_ID}`);
	await expect(page.getByTestId('plan-body')).toContainText('Guard the start-up path');
	await expect(page.getByRole('button', { name: 'Approve', exact: true })).toBeVisible();
	await page.getByRole('button', { name: 'Approve', exact: true }).click();
	await expect(page.getByText('Plan approved: implementation started.')).toBeVisible();
	// The run no longer waits, so the plan is gone.
	await expect(page.getByTestId('plan-body')).toHaveCount(0);
});

test('a pull request shows notes, checks and the inline diff; red checks refuse the merge', async ({
	page
}) => {
	await installFakeSandbox(page);
	await page.goto(`${BASE}?task=${encodeURIComponent('pr:e2e-org/app#51')}`);
	await expect(page.getByTestId('checks-summary')).toContainText('1 failed (lint)');
	await page.getByRole('button', { name: 'Approve & merge' }).click();
	const steps = page.getByRole('list', { name: 'Approve and merge progress' });
	await expect(steps).toContainText('Checks are failing: lint.');
	await expect(steps.getByText('Squash and merge')).toBeVisible();
	// Still open: nothing was merged.
	await expect(
		page.getByRole('listitem', { name: /Pull requests to review: Broken change/ })
	).toBeVisible();
});

test('Approve & merge approves, merges and the pull request leaves the list', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 800 });
	await installFakeSandbox(page);
	await page.goto(`${BASE}?task=${encodeURIComponent('pr:e2e-org/app#50')}`);
	const notes = page.getByTestId('review-notes');
	await expect(notes.locator('strong')).toHaveText('Looks good.');
	await expect(page.getByRole('link', { name: 'review-pull-requests #88888888' })).toBeVisible();
	await expect(page.getByTestId('checks-summary')).toContainText('2 passed');
	await expect(page.getByText('src/start.ts')).toBeVisible();
	await expect(page.locator('.ez-diff__line--add').first()).toContainText('config');
	await expect(page.locator('.ez-diff__line--del').first()).toContainText('boot(null)');
	await expect(page.getByText('Too large to show inline').first()).toBeVisible();
	if (process.env.E2E_SHOTS) await page.screenshot({ path: `${process.env.E2E_SHOTS}/pr.png` });
	await page.getByRole('button', { name: 'Approve & merge' }).click();
	await expect(page.getByTestId('task-done')).toContainText('Approved and merged #50.');
	await expect(page.getByRole('listitem', { name: /Pull requests to review: Fix #7/ })).toHaveCount(
		0
	);
});
