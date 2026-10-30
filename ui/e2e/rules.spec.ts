import { mkdir, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { expect, test } from '@playwright/test';
import { E2E_PROJECT } from './constants.ts';
import { installFakeSandbox } from './fake-sandbox.ts';
import { REPO_DIR } from './stubs/repo-files.ts';

// The committed state must show on the page without a reload. These run the real remote functions
// against an on-disk stand-in for the repository (e2e/stubs/repo-files.ts).
const URL = `/projects/${E2E_PROJECT}/workflows`;
const repoDir = path.join(REPO_DIR, 'e2e-org/app');

test.beforeEach(async () => {
	await rm(repoDir, { recursive: true, force: true });
});

async function seedRule(id: string, cond: string) {
	await mkdir(path.join(repoDir, '.ez'), { recursive: true });
	const wf = {
		version: 1,
		workflows: {
			[id]: { on: ['push'], if: cond, start: 'e', nodes: { e: { type: 'end', status: 'done' } } }
		}
	};
	await writeFile(path.join(repoDir, '.ez/workflows.toml'), JSON.stringify(wf));
}

test('enabling a template on a first-run repo shows the rule without a reload', async ({
	page
}) => {
	await installFakeSandbox(page);
	await page.goto(URL);
	await expect(page.getByText('There are no rules yet.')).toBeVisible();
	await page.getByRole('checkbox', { name: 'Review pull requests' }).check();
	await page.getByRole('button', { name: 'Enable selected' }).click();
	await expect(page.getByRole('listitem', { name: 'Rule review-pull-requests' })).toBeVisible({
		timeout: 10_000
	});
	await expect(page.getByText('There are no rules yet.')).toHaveCount(0);
});

test('editing a rule shows the new value and a second commit works', async ({ page }) => {
	await seedRule('wf', 'one');
	await installFakeSandbox(page);
	await page.goto(URL);
	const cond = page.getByLabel('Only if, for wf');
	await cond.fill('two');
	await page
		.getByRole('button', { name: /^Save|Commit/ })
		.first()
		.click();
	await expect(page.getByText(/Committed to main/)).toBeVisible();
	await expect(page.locator('.ez-rules__sentence code', { hasText: 'two' })).toBeVisible();
	await page.getByLabel('Only if, for wf').fill('three');
	await page
		.getByRole('button', { name: /^Save|Commit/ })
		.first()
		.click();
	await expect(page.locator('.ez-rules__sentence code', { hasText: 'three' })).toBeVisible();
	await expect(page.getByText(/changed on main/)).toHaveCount(0);
});
