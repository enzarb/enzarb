import { page as browser } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { blankMode } from '#lib/modes-types.ts';

// The pages' own query must pick up the new blob sha after a save, whatever repo key it was
// opened with, or the next save conflicts. The remote functions are faked with one query store.
const h = vi.hoisted(() => ({
	sha: 'a',
	args: [] as unknown[],
	saves: [] as { baseSha: string | null }[]
}));

vi.mock('$app/state', () => ({ page: { params: { name: 'alpha' } } }));

function query<T>(load: () => T) {
	return (args: unknown) => {
		h.args.push(args);
		let current = $state.raw(load());
		return {
			get current() {
				return current;
			},
			error: undefined,
			refresh: async () => {
				current = load();
			}
		};
	};
}

const save = vi.hoisted(() =>
	vi.fn(async (input: { baseSha: string | null }) => {
		h.saves.push(input);
		h.sha = `${h.sha}x`;
		return { ok: true as const, unchanged: false, commitUrl: null };
	})
);

vi.mock('#lib/workflows.remote.ts', () => ({
	getWorkflows: query(() => ({
		ok: true as const,
		repos: ['github:acme/w'],
		repo: 'github:acme/w',
		branch: 'main',
		path: '.ez/workflows.toml',
		sha: h.sha,
		text: `version = 1 # ${h.sha}\n`,
		file: null,
		report: [],
		modesToml: null,
		modeIds: []
	})),
	checkWorkflowsText: vi.fn(async () => ({ ok: true, diagnostics: [], file: null })),
	renderWorkflowsFile: vi.fn(async () => ({ ok: true, text: '', diagnostics: [] })),
	saveWorkflowsFile: save,
	listRepoEvents: vi.fn(async () => []),
	dryRunWorkflows: vi.fn()
}));

vi.mock('#lib/modes.remote.ts', () => ({
	getModes: query(() => ({
		ok: true as const,
		repos: ['github:acme/w'],
		repo: 'github:acme/w',
		branch: 'main',
		path: '.ez/modes.toml',
		sha: h.sha,
		text: '[modes.plan]\n',
		doc: { defaultMode: 'plan', modes: [{ ...blankMode('plan'), originalId: 'plan' }] },
		issues: [],
		parseError: null,
		catalog: {
			builtinTools: [],
			hookEvents: [],
			permissionValues: [],
			onFailureValues: [],
			templates: []
		}
	})),
	parseModesText: vi.fn(),
	renderModesText: vi.fn(),
	validateModes: vi.fn(async () => ({ ok: true, issues: [] })),
	saveModesFile: save
}));

import Workflows from './workflows/+page.svelte';
import Modes from './modes/+page.svelte';

const commit = () => browser.getByRole('button', { name: 'Commit to main' });

beforeEach(() => {
	h.sha = 'a';
	h.args = [];
	h.saves = [];
});

describe('designer pages', () => {
	it('workflows: a second save sends the sha of the first commit', async () => {
		render(Workflows);
		const editor = browser.getByRole('textbox', { name: '.ez/workflows.toml' });
		await editor.fill('version = 1\n# one\n');
		await commit().click();
		await expect.poll(() => h.saves.length).toBe(1);
		await expect.element(editor).toHaveValue('version = 1 # ax\n');
		await editor.fill('version = 1\n# two\n');
		await commit().click();
		await expect.poll(() => h.saves.length).toBe(2);
		expect(h.saves.map((s) => s.baseSha)).toEqual(['a', 'ax']);
		expect(new Set(h.args.map((a) => JSON.stringify(a))).size).toBe(1);
	});

	it('modes: a second save sends the sha of the first commit', async () => {
		render(Modes);
		await browser.getByRole('button', { name: /^plan/ }).click();
		await browser.getByRole('textbox', { name: 'Description' }).fill('one');
		await commit().click();
		await expect.poll(() => h.saves.length).toBe(1);
		await expect.element(commit()).toBeDisabled();
		await browser.getByRole('button', { name: /^plan/ }).click();
		await browser.getByRole('textbox', { name: 'Description' }).fill('two');
		await commit().click();
		await expect.poll(() => h.saves.length).toBe(2);
		expect(h.saves.map((s) => s.baseSha)).toEqual(['a', 'ax']);
	});
});
