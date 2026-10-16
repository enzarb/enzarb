import { expect, test, type Page } from '@playwright/test';
import { E2E_PROJECT } from './constants.ts';
import { installFakeSandbox } from './fake-sandbox.ts';

function collectErrors(page: Page): string[] {
	const errors: string[] = [];
	page.on('pageerror', (e) => errors.push(`pageerror: ${e.stack ?? e.message}`));
	page.on('console', (m) => {
		if (m.type() === 'error' && !m.text().includes('/live/v1/stream'))
			errors.push(`console: ${m.text()} ${m.location().url}`);
	});
	return errors;
}

/** A live stream that keeps reporting changes, as enzarb-realtime does while work happens. */
async function installFakeLive(page: Page) {
	await page.routeWebSocket(/\/live\/v1\/stream/, (ws) => {
		let n = 0;
		const timer = setInterval(() => {
			n++;
			ws.send(
				JSON.stringify(n % 3 === 0 ? { topic: 'resync' } : { topic: 'task', id: (n % 2) + 1 })
			);
		}, 250);
		ws.onClose(() => clearInterval(timer));
	});
}

for (const path of [`/projects/${E2E_PROJECT}`, `/projects/${E2E_PROJECT}/settings`, '/tasks']) {
	test(`${path} renders without client errors`, async ({ page }) => {
		const errors = collectErrors(page);
		await installFakeSandbox(page);
		await installFakeLive(page);
		await page.goto(path);
		await page.waitForTimeout(1500);
		await page.waitForTimeout(1000);
		expect(errors).toEqual([]);
	});
}

test('the project board does not scroll sideways on a phone', async ({ page }) => {
	await page.setViewportSize({ width: 360, height: 740 });
	await installFakeSandbox(page);
	await installFakeLive(page);
	await page.goto(`/projects/${E2E_PROJECT}/tasks`);
	const board = page.getByRole('group', { name: 'Task board' });
	await expect(board).toBeVisible();
	await page.waitForTimeout(1500);
	// Columns stack: every column starts at the board's left edge, one below the other.
	const boxes = await board.locator('.ez-board__col').evaluateAll((cols) =>
		cols.map((c) => {
			const r = c.getBoundingClientRect();
			return { x: r.x, y: r.y };
		})
	);
	expect(boxes.length).toBeGreaterThan(1);
	for (const [i, b] of boxes.entries()) {
		expect(b.x).toBe(boxes[0].x);
		if (i > 0) expect(b.y).toBeGreaterThan(boxes[i - 1].y);
	}
	const { scrollWidth, clientWidth } = await page.evaluate(() => ({
		scrollWidth: document.documentElement.scrollWidth,
		clientWidth: document.documentElement.clientWidth
	}));
	expect(scrollWidth).toBeLessThanOrEqual(clientWidth);
});

test('serves the web app manifest and its icons without a session', async ({
	browser,
	baseURL
}) => {
	const ctx = await browser.newContext({ baseURL, storageState: { cookies: [], origins: [] } });
	try {
		const res = await ctx.request.get('/manifest.webmanifest');
		expect(res.ok()).toBe(true);
		const manifest = await res.json();
		expect(manifest.display).toBe('standalone');
		expect(manifest.start_url).toBe('/work');
		for (const icon of manifest.icons) expect((await ctx.request.get(icon.src)).ok()).toBe(true);
	} finally {
		await ctx.close();
	}
});

test('settings render the notifications section', async ({ page }) => {
	const errors = collectErrors(page);
	await installFakeLive(page);
	await page.goto('/settings');
	await expect(page.getByRole('heading', { name: 'Notifications' })).toBeVisible();
	// The e2e server has no VAPID keys.
	await expect(
		page.getByText('Push notifications are not configured on this server.')
	).toBeVisible();
	expect(errors).toEqual([]);
});

test('saving repositories in project settings succeeds', async ({ page }) => {
	const errors = collectErrors(page);
	await installFakeSandbox(page);
	await installFakeLive(page);
	await page.goto(`/projects/${E2E_PROJECT}/settings`);
	await page.getByRole('button', { name: 'Edit repositories' }).click();
	await page.getByRole('checkbox', { name: 'e2e-org/lib' }).check();
	await page.getByRole('button', { name: 'Save repositories' }).click();
	await expect(page.getByRole('link', { name: 'e2e-org/lib' })).toBeVisible();
	await expect(page.getByText('Could not save the repositories.')).toHaveCount(0);
	expect(errors).toEqual([]);
});

test('hovering links does not crash', async ({ page }) => {
	const errors = collectErrors(page);
	await installFakeSandbox(page);
	await installFakeLive(page);
	for (const start of [
		`/projects/${E2E_PROJECT}`,
		`/projects/${E2E_PROJECT}/settings`,
		'/tasks',
		`/projects/${E2E_PROJECT}/tasks`,
		'/projects'
	]) {
		await page.goto(start);
		await page.waitForTimeout(1500);
		for (const link of await page.getByRole('link').all()) {
			if (await link.isVisible()) await link.hover();
			await page.waitForTimeout(300);
		}
	}
	expect(errors).toEqual([]);
});

test('client-side navigation does not crash', async ({ page }) => {
	const errors = collectErrors(page);
	await installFakeSandbox(page);
	await installFakeLive(page);
	await page.goto('/projects');
	await page.waitForTimeout(1500);
	await page
		.getByRole('link', { name: /E2E project/ })
		.first()
		.click();
	await expect(page.getByRole('heading', { name: 'Repositories' })).toBeVisible();
	await page.getByRole('link', { name: 'Settings' }).last().click();
	await expect(page.getByRole('button', { name: 'Edit repositories' })).toBeVisible();
	await page.getByRole('link', { name: 'Overview' }).click();
	await page.getByRole('link', { name: /Tasks/ }).hover();
	await page.waitForTimeout(500);
	await page.getByRole('link', { name: /Tasks/ }).click();
	await page.waitForTimeout(1500);
	await page.getByRole('link', { name: 'Projects' }).click();
	await page
		.getByRole('link', { name: /E2E project/ })
		.first()
		.click();
	await page.waitForTimeout(1000);
	expect(errors).toEqual([]);
});
