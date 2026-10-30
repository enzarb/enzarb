import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import ModesDesigner, { type DesignerApi } from './ModesDesigner.svelte';
import { blankMode, type ModesCatalog, type ModesFile } from '#lib/modes-types.ts';

const catalog: ModesCatalog = {
	builtinTools: [
		{ name: 'Read', kind: 'read', readOnly: true },
		{ name: 'Edit', kind: 'edit', readOnly: false }
	],
	hookEvents: [{ name: 'post_edit', glob: true, matcher: false, description: 'after an edit' }],
	permissionValues: ['allow', 'ask', 'deny'],
	onFailureValues: ['ignore', 'block', 'fix_turn'],
	templates: [
		{ set: 'minimal', mode: { ...blankMode('plan'), description: 'Plan it' } },
		{ set: 'minimal', mode: { ...blankMode('build'), description: 'Build it' } }
	]
};

const file = (over: Partial<ModesFile> = {}): ModesFile => ({
	repo: 'github:acme/w',
	branch: 'main',
	path: '.ez/modes.toml',
	sha: 'a'.repeat(40),
	text: '[modes.plan]\n',
	doc: {
		defaultMode: 'plan',
		modes: [
			{ ...blankMode('plan'), originalId: 'plan', description: 'Plan' },
			{ ...blankMode('build'), originalId: 'build', description: 'Build' }
		]
	},
	issues: [],
	parseError: null,
	catalog,
	...over
});

function fakeApi(over: Partial<DesignerApi> = {}): DesignerApi {
	return {
		validate: vi.fn(async () => ({ ok: true as const, issues: [] })),
		parse: vi.fn(async () => ({ ok: true as const, doc: file().doc, issues: [] })),
		render: vi.fn(async () => ({ ok: true as const, text: '[modes.plan]\ndescription = "x"\n' })),
		save: vi.fn(async () => ({
			ok: true as const,
			unchanged: false,
			commitUrl: 'https://github.com/c'
		})),
		reload: vi.fn(),
		...over
	};
}

const commit = () => page.getByRole('button', { name: 'Commit to main' });

describe('ModeForm basic/advanced split', () => {
	it('keeps the extras inside a collapsed Advanced section', async () => {
		render(ModesDesigner, { file: file(), api: fakeApi(), saved: vi.fn() });
		await expect.element(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
		await expect.element(page.getByRole('textbox', { name: 'Model' })).toBeVisible();
		const adv = page.getByText('Advanced', { exact: true });
		await expect.element(adv).toBeVisible();
		await expect.element(page.getByRole('textbox', { name: 'Profile' })).not.toBeInTheDocument();
		await adv.click();
		await expect.element(page.getByRole('textbox', { name: 'Profile' })).toBeVisible();
	});
});

describe('ModesDesigner', () => {
	it('lists the modes, edits the selected one and saves the document', async () => {
		const api = fakeApi();
		const saved = vi.fn();
		render(ModesDesigner, { file: file(), api, saved });
		await expect.element(page.getByRole('button', { name: /plan.*default/ })).toBeInTheDocument();
		await expect.element(commit()).toBeDisabled();

		await page.getByRole('button', { name: /^build/ }).click();
		await page.getByRole('textbox', { name: 'Description' }).fill('Build carefully');
		await expect.poll(() => vi.mocked(api.validate).mock.calls.length).toBeGreaterThan(0);
		await expect.element(commit()).toBeEnabled();
		await page.getByRole('textbox', { name: 'Commit message' }).fill('feat: tune build');
		await commit().click();

		await expect.poll(() => vi.mocked(api.save).mock.calls.length).toBe(1);
		const input = vi.mocked(api.save).mock.calls[0][0];
		expect(input.message).toBe('feat: tune build');
		expect(input.doc?.modes[1].description).toBe('Build carefully');
		expect(input.text).toBeUndefined();
		await expect
			.poll(() => saved.mock.calls[0]?.[0])
			.toEqual({ unchanged: false, commitUrl: 'https://github.com/c' });
	});

	it('renames a mode together with its references', async () => {
		const api = fakeApi();
		render(ModesDesigner, { file: file(), api, saved: vi.fn() });
		await page.getByRole('textbox', { name: 'Id' }).fill('scout');
		await expect.element(page.getByRole('button', { name: /scout.*default/ })).toBeInTheDocument();
	});

	it('stops at validation when there are errors and lists them', async () => {
		const issues = [
			{ path: 'modes[1].next[0]', message: 'unknown mode `nope`', severity: 'error' as const }
		];
		const api = fakeApi({ validate: vi.fn(async () => ({ ok: true as const, issues })) });
		render(ModesDesigner, { file: file(), api, saved: vi.fn() });
		await page.getByRole('textbox', { name: 'Description' }).fill('changed');
		await expect.element(page.getByText('unknown mode `nope`')).toBeInTheDocument();
		await expect.element(commit()).toBeDisabled();
		expect(api.save).not.toHaveBeenCalled();
	});

	it('shows no errors on a repo without modes until something is picked, and cannot commit', async () => {
		const issues = [
			{ path: 'modes', message: 'Define at least one mode', severity: 'error' as const }
		];
		const api = fakeApi({ validate: vi.fn(async () => ({ ok: true as const, issues })) });
		render(ModesDesigner, {
			file: file({ sha: null, text: null, doc: { defaultMode: '', modes: [] }, issues }),
			api,
			saved: vi.fn()
		});
		await expect.element(page.getByText('Start your modes')).toBeInTheDocument();
		await expect.element(page.getByText('Define at least one mode')).not.toBeInTheDocument();
		await expect.element(commit()).toBeDisabled();
	});

	it('offers Reload after a conflict', async () => {
		const api = fakeApi({
			save: vi.fn(async () => ({
				ok: false as const,
				code: 'conflict',
				message: 'The file changed on main.'
			}))
		});
		render(ModesDesigner, { file: file(), api, saved: vi.fn() });
		await page.getByRole('textbox', { name: 'Description' }).fill('changed');
		await commit().click();
		await expect.element(page.getByText('The file changed on main.')).toBeInTheDocument();
		await page.getByRole('button', { name: 'Reload the file' }).click();
		expect(api.reload).toHaveBeenCalled();
	});

	it('starts a missing file from a template', async () => {
		const api = fakeApi();
		render(ModesDesigner, {
			file: file({ sha: null, text: null, doc: { defaultMode: '', modes: [] } }),
			api,
			saved: vi.fn()
		});
		await expect
			.element(page.getByText('There is no .ez/modes.toml on main yet.', { exact: false }))
			.toBeInTheDocument();
		await page.getByRole('button', { name: 'minimal template' }).click();
		await expect.element(page.getByRole('button', { name: /plan.*default/ })).toBeInTheDocument();
		await expect.element(commit()).toBeEnabled();
	});

	it('switches to TOML through tracks and saves the text', async () => {
		const api = fakeApi();
		render(ModesDesigner, { file: file(), api, saved: vi.fn() });
		await page.getByRole('tab', { name: 'TOML' }).click();
		const editor = page.getByRole('textbox', { name: '.ez/modes.toml' });
		await expect.element(editor).toHaveValue('[modes.plan]\ndescription = "x"\n');
		expect(vi.mocked(api.render).mock.calls[0][1]).toBe('[modes.plan]\n');
		await editor.fill('# edited\n');
		await commit().click();
		await expect
			.poll(() => vi.mocked(api.save).mock.calls[0]?.[0])
			.toEqual({ text: '# edited\n', message: '' });
		expect(api.parse).toHaveBeenCalledWith('# edited\n');
	});

	it('opens a broken file in the TOML tab', async () => {
		render(ModesDesigner, {
			file: file({
				text: 'bad =',
				parseError: 'parsing modes.toml',
				doc: { defaultMode: '', modes: [] }
			}),
			api: fakeApi(),
			saved: vi.fn()
		});
		await expect.element(page.getByText('The file is not valid modes TOML')).toBeInTheDocument();
		await expect
			.element(page.getByRole('textbox', { name: '.ez/modes.toml' }))
			.toHaveValue('bad =');
	});
});
