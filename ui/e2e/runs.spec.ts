import { expect, test } from '@playwright/test';
import { E2E_PROJECT } from './constants.ts';
import { installFakeSandbox } from './fake-sandbox.ts';
import { E2E_RUN_ID } from './stubs/tracks.ts';

// The Runs tab against the real remote functions; enzarb-tracks is the stand-in in
// e2e/stubs/tracks.ts. Stop must show the cancelled run without a reload (server-side refresh).
const BASE = `/projects/${E2E_PROJECT}/runs`;

test('lists the run, shows each node and stops it', async ({ page }) => {
	await installFakeSandbox(page);
	await page.goto(BASE);
	const row = page.getByRole('link', { name: /fix-issue/ });
	await expect(row).toBeVisible();
	await expect(row).toContainText('running');
	await row.click();
	await expect(page).toHaveURL(`${BASE}/${E2E_RUN_ID}`);

	const triage = page.getByRole('listitem', { name: 'Node triage' });
	await expect(triage).toContainText('Triage issue #7');
	await expect(triage).toContainText('mode triage');
	await expect(triage).toContainText('11 in, 22 out');
	await expect(triage.getByRole('link', { name: 'ez/fix-issue/x' })).toHaveAttribute(
		'href',
		'https://github.com/e2e-org/app/tree/ez/fix-issue/x'
	);
	await expect(page.getByRole('listitem', { name: 'Node build' })).toContainText(
		'mise run build -- go'
	);

	await page.getByRole('button', { name: 'Stop', exact: true }).click();
	await page.getByRole('button', { name: 'Stop run' }).click();
	await expect(page.getByText('cancelled').first()).toBeVisible({ timeout: 10_000 });
	await expect(page.getByRole('button', { name: 'Stop', exact: true })).toHaveCount(0);
});

test('an unknown run is a clear error', async ({ page }) => {
	await installFakeSandbox(page);
	await page.goto(`${BASE}/22222222-2222-4222-8222-222222222222`);
	await expect(page.getByText('Could not load the run')).toBeVisible();
});
