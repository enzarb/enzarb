import { defineConfig, devices } from '@playwright/test';
import { E2E_ORIGIN, STORAGE_STATE } from './e2e/constants.ts';

export default defineConfig({
	testDir: 'e2e',
	testMatch: '*.spec.ts',
	fullyParallel: false,
	workers: 1,
	forbidOnly: !!process.env.CI,
	retries: process.env.CI ? 1 : 0,
	reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
	use: {
		baseURL: E2E_ORIGIN,
		storageState: STORAGE_STATE,
		// For E2E_PROD=1 (see e2e/serve.ts): what the Gateway adds in front of the production server.
		extraHTTPHeaders: { 'X-Forwarded-Proto': 'http' },
		trace: 'on-first-retry'
	},
	projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
	webServer: {
		// Starts Postgres, seeds the session behind `storageState`, then vite dev (see the script).
		command: 'node e2e/serve.ts',
		url: `${E2E_ORIGIN}/healthz`,
		reuseExistingServer: false,
		timeout: 180_000,
		stdout: 'pipe',
		stderr: 'pipe'
	}
});
