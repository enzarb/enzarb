import { page, userEvent } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import type { ComponentProps } from 'svelte';
import { render } from 'vitest-browser-svelte';
import NewProjectForm, { type RepositoryChoices } from './NewProjectForm.svelte';
import type { CreateProjectsOutcome } from '#lib/onboarding.ts';

const choices = (over: Partial<RepositoryChoices> = {}): RepositoryChoices => ({
	repositories: [
		{ repo: 'github:acme/web', accountLogin: 'acme', private: true, defaultBranch: 'main' },
		{ repo: 'github:acme/api', accountLogin: 'acme', private: false, defaultBranch: 'main' },
		{ repo: 'github:me/dotfiles', accountLogin: 'me', private: false, defaultBranch: 'main' }
	],
	taken: { 'github:acme/api': 'backend' },
	projectNames: ['backend'],
	problem: null,
	...over
});

const ok: CreateProjectsOutcome = { ok: true, results: [] };

function setup(over: Partial<RepositoryChoices> = {}, create = vi.fn(async () => ok)) {
	render(NewProjectForm, {
		choices: choices(over),
		refreshHref: '/login/github?refresh=1',
		create
	});
	return create;
}

async function pick(...names: RegExp[]) {
	const input = page.getByRole('combobox', { name: 'Repositories' });
	await input.click();
	await userEvent.keyboard('{ArrowDown}');
	for (const n of names) await page.getByRole('option', { name: n }).click();
	await userEvent.keyboard('{Escape}');
}

describe('NewProjectForm', () => {
	it('lists repositories grouped by account, marks private, disables ones already in a project', async () => {
		setup();
		await page.getByRole('combobox', { name: 'Repositories' }).click();
		await userEvent.keyboard('{ArrowDown}');
		await expect.element(page.getByRole('group', { name: 'acme' })).toBeInTheDocument();
		await expect.element(page.getByRole('group', { name: 'me', exact: true })).toBeInTheDocument();
		await expect
			.element(page.getByRole('option', { name: /acme\/web/ }))
			.toHaveTextContent('private');
		const taken = page.getByRole('option', { name: /acme\/api/ });
		await expect.element(taken).toHaveAttribute('aria-disabled', 'true');
		await expect.element(taken).toHaveTextContent('in backend');
		await taken.click({ force: true });
		await expect
			.element(page.getByRole('button', { name: 'Remove acme/api' }))
			.not.toBeInTheDocument();
	});

	it('shows the default of 2 and sends maxRuns only when changed', async () => {
		const create = setup();
		await pick(/acme\/web/);
		await page.getByRole('textbox', { name: 'Project name' }).fill('x');
		const input = page.getByRole('spinbutton', { name: 'Concurrent runs' });
		await expect.element(input).toHaveValue(2);
		await input.fill('5');
		await page.getByRole('button', { name: 'Create project' }).click();
		expect(create).toHaveBeenCalledWith(
			[expect.objectContaining({ name: 'x', maxRuns: 5 })],
			expect.anything()
		);
	});

	it('blocks submit for an out-of-range maxRuns', async () => {
		setup();
		await pick(/acme\/web/);
		await page.getByRole('textbox', { name: 'Project name' }).fill('x');
		await page.getByRole('spinbutton', { name: 'Concurrent runs' }).fill('17');
		await expect.element(page.getByRole('button', { name: 'Create project' })).toBeDisabled();
	});

	it('selects repositories with the keyboard and skips the disabled one', async () => {
		setup();
		await page.getByRole('combobox', { name: 'Repositories' }).click();
		await userEvent.keyboard('{ArrowDown}');
		await userEvent.keyboard('{Enter}'); // acme/web (first enabled)
		await expect.element(page.getByRole('button', { name: 'Remove acme/web' })).toBeInTheDocument();
		await userEvent.keyboard('{ArrowDown}{ArrowDown}{Enter}');
		await expect
			.element(page.getByRole('button', { name: 'Remove acme/api' }))
			.not.toBeInTheDocument();
		await expect
			.element(page.getByRole('button', { name: 'Remove me/dotfiles' }))
			.toBeInTheDocument();
	});

	it('creates one project with all selected repositories under one name', async () => {
		const create = setup();
		await pick(/acme\/web/, /me\/dotfiles/);
		const submit = page.getByRole('button', { name: 'Create project' });
		await expect.element(submit).toBeDisabled(); // no name yet
		await page.getByRole('textbox', { name: 'Project name' }).fill('tools');
		await expect.element(submit).toBeEnabled();
		await submit.click();
		expect(create).toHaveBeenCalledWith(
			[
				{
					name: 'tools',
					displayName: '',
					repositories: ['github:acme/web', 'github:me/dotfiles']
				}
			],
			{ issues: true, changeRequests: true, mentions: true }
		);
	});

	it('switches to one project per repository with editable, slugified names', async () => {
		const create = setup();
		await pick(/acme\/web/, /me\/dotfiles/);
		await page.getByRole('radio', { name: /One project per repository/ }).click();
		await expect
			.element(page.getByRole('textbox', { name: 'Name for acme/web' }))
			.toHaveValue('web');
		await expect
			.element(page.getByRole('textbox', { name: 'Name for me/dotfiles' }))
			.toHaveValue('dotfiles');

		await page.getByRole('textbox', { name: 'Name template' }).fill('{owner}_{repo}');
		await expect
			.element(page.getByRole('textbox', { name: 'Name for me/dotfiles' }))
			.toHaveValue('me-dotfiles');

		await page.getByRole('textbox', { name: 'Name for acme/web' }).fill('front');
		await page.getByRole('button', { name: 'Create 2 projects' }).click();
		expect(create).toHaveBeenCalledWith(
			[
				{ name: 'front', repositories: ['github:acme/web'] },
				{ name: 'me-dotfiles', repositories: ['github:me/dotfiles'] }
			],
			expect.anything()
		);
	});

	it('blocks creation for invalid, repeated and existing names', async () => {
		setup();
		await pick(/acme\/web/, /me\/dotfiles/);
		await page.getByRole('radio', { name: /One project per repository/ }).click();
		const submit = page.getByRole('button', { name: 'Create 2 projects' });
		await page.getByRole('textbox', { name: 'Name for acme/web' }).fill('Bad Name');
		await expect.element(submit).toBeDisabled();
		await expect.element(page.getByText(/lowercase letters/).first()).toBeInTheDocument();

		await page.getByRole('textbox', { name: 'Name for acme/web' }).fill('same');
		await page.getByRole('textbox', { name: 'Name for me/dotfiles' }).fill('same');
		await expect.element(submit).toBeDisabled();
		await expect.element(page.getByText(/used more than once/).first()).toBeInTheDocument();

		await page.getByRole('textbox', { name: 'Name for me/dotfiles' }).fill('backend');
		await expect.element(submit).toBeDisabled();
		await expect.element(page.getByText(/already exists/)).toBeInTheDocument();

		await page.getByRole('textbox', { name: 'Name for me/dotfiles' }).fill('dots');
		await expect.element(submit).toBeEnabled();
	});

	it('shows a refresh link and blocks the form when the snapshot is stale', async () => {
		setup({
			problem: {
				code: 'stale-snapshot',
				message: 'Your repository list is more than 24 hours old.'
			}
		});
		await expect.element(page.getByRole('alert')).toHaveTextContent('more than 24 hours old');
		await expect
			.element(page.getByRole('link', { name: 'Refresh repositories' }))
			.toHaveAttribute('href', '/login/github?refresh=1');
		await expect.element(page.getByRole('button', { name: 'Create project' })).toBeDisabled();
	});

	it('shows the server’s refusal with a refresh link', async () => {
		const create = vi.fn(async (): Promise<CreateProjectsOutcome> => ({
			ok: false,
			code: 'stale-snapshot',
			message: 'Refresh it.'
		}));
		setup({}, create);
		await pick(/acme\/web/);
		await page.getByRole('textbox', { name: 'Project name' }).fill('x');
		await page.getByRole('button', { name: 'Create project' }).click();
		await expect.element(page.getByRole('alert')).toHaveTextContent('Refresh it.');
		await expect
			.element(page.getByRole('link', { name: 'Refresh repositories' }))
			.toBeInTheDocument();
	});

	it('keeps the form and selections and shows reasons when every project failed', async () => {
		const create = vi.fn(async (): Promise<CreateProjectsOutcome> => ({
			ok: true,
			results: [
				{
					name: 'x',
					status: 'failed',
					repositories: ['github:acme/web'],
					reason: 'cluster said no'
				}
			]
		}));
		setup({}, create);
		await pick(/acme\/web/);
		await page.getByRole('textbox', { name: 'Project name' }).fill('x');
		await page.getByRole('button', { name: 'Create project' }).click();
		await expect.element(page.getByRole('alert')).toHaveTextContent('cluster said no');
		await expect.element(page.getByRole('button', { name: 'Remove acme/web' })).toBeInTheDocument();
		await expect.element(page.getByRole('textbox', { name: 'Project name' })).toHaveValue('x');
	});

	it('blocks more than 50 projects per request', async () => {
		const many = Array.from({ length: 51 }, (_, i) => ({
			repo: `github:big/r${i}`,
			accountLogin: 'big',
			private: false,
			defaultBranch: 'main'
		}));
		setup({ repositories: many, taken: {}, projectNames: [] });
		const input = page.getByRole('combobox', { name: 'Repositories' });
		await input.click();
		await userEvent.keyboard('{ArrowDown}');
		for (let i = 0; i < 51; i++) await userEvent.keyboard('{Enter}{ArrowDown}');
		await page.getByRole('radio', { name: /One project per repository/ }).click();
		await expect
			.element(page.getByText('At most 50 projects per request').first())
			.toBeInTheDocument();
		await expect.element(page.getByRole('button', { name: 'Create 51 projects' })).toBeDisabled();
	});
});

describe('NewProjectForm: create a new GitHub repository', () => {
	type Starter = NonNullable<ComponentProps<typeof NewProjectForm>['startCreateRepo']>;
	function setupRepo(
		props: Partial<ComponentProps<typeof NewProjectForm>> = {},
		create = vi.fn(async (): Promise<CreateProjectsOutcome> => ok)
	) {
		const startCreateRepo = vi.fn<Starter>(async () => ({
			ok: true,
			href: '/login/github?purpose=create-repo&return=%2Fprojects%2Fnew'
		}));
		const navigate = vi.fn();
		render(NewProjectForm, {
			choices: choices(),
			refreshHref: '/login/github?refresh=1',
			create,
			startCreateRepo,
			navigate,
			...props
		});
		return { create, startCreateRepo, navigate };
	}
	const steps = () => page.getByRole('list', { name: 'New repository progress' });

	it('starts the re-authorization with owner, name (default: project name), visibility and description', async () => {
		const { startCreateRepo, navigate } = setupRepo();
		const button = page.getByRole('button', { name: 'Create repository on GitHub' });
		await expect.element(button).toBeDisabled(); // no name yet
		await page.getByRole('textbox', { name: 'Project name' }).fill('tools');
		await page.getByRole('radio', { name: 'Public' }).click();
		await page.getByRole('textbox', { name: 'Description' }).fill('my tools');
		await button.click();
		expect(startCreateRepo).toHaveBeenCalledWith({
			owner: 'acme',
			name: 'tools',
			visibility: 'public',
			description: 'my tools'
		});
		expect(navigate).toHaveBeenCalledWith(
			'/login/github?purpose=create-repo&return=%2Fprojects%2Fnew'
		);
	});

	it('defaults to private and blocks invalid names', async () => {
		const { startCreateRepo } = setupRepo();
		const button = page.getByRole('button', { name: 'Create repository on GitHub' });
		await page.getByRole('textbox', { name: 'Repository name' }).fill('a/b');
		await expect.element(button).toBeDisabled();
		await page.getByRole('textbox', { name: 'Repository name' }).fill('ok-name');
		await button.click();
		expect(startCreateRepo).toHaveBeenCalledWith(
			expect.objectContaining({ name: 'ok-name', visibility: 'private' })
		);
	});

	it('shows a refusal from the server and does not navigate', async () => {
		const startCreateRepo = vi.fn<Starter>(async () => ({ ok: false, message: 'Choose an owner' }));
		const { navigate } = setupRepo({ startCreateRepo });
		await page.getByRole('textbox', { name: 'Repository name' }).fill('x');
		await page.getByRole('button', { name: 'Create repository on GitHub' }).click();
		await expect.element(page.getByText('Choose an owner')).toBeInTheDocument();
		expect(navigate).not.toHaveBeenCalled();
	});

	it('preselects the new repository, shows the checklist, and mixes in existing ones', async () => {
		const { create } = setupRepo({ newRepo: 'github:acme/web', repoResult: { code: 'created' } });
		await expect.element(page.getByRole('button', { name: 'Remove acme/web' })).toBeInTheDocument();
		await expect.element(steps().getByText('Create repository')).toBeInTheDocument();
		await expect.element(steps().getByText('Install on repository')).toBeInTheDocument();
		await expect.element(steps().getByText('Create project')).toBeInTheDocument();
		await pick(/me\/dotfiles/);
		await page.getByRole('textbox', { name: 'Project name' }).fill('tools');
		await page.getByRole('button', { name: 'Create project' }).click();
		expect(create).toHaveBeenCalledWith(
			[expect.objectContaining({ repositories: ['github:acme/web', 'github:me/dotfiles'] })],
			expect.anything()
		);
	});

	it('reports a GitHub refusal in the checklist with its status', async () => {
		setupRepo({ repoResult: { code: 'forbidden', status: 403 } });
		const first = steps().getByRole('listitem').first();
		await expect.element(first).toHaveAttribute('data-state', 'failed');
		await expect.element(first).toHaveTextContent('GitHub status 403');
	});

	it('fails the install step when the repository was created but not installed', async () => {
		setupRepo({
			newRepo: 'github:acme/web',
			repoResult: { code: 'created-not-installed', status: 403 }
		});
		const items = steps().getByRole('listitem');
		await expect.element(items.nth(0)).toHaveAttribute('data-state', 'done');
		await expect.element(items.nth(1)).toHaveAttribute('data-state', 'failed');
	});

	it('shows the identity-mismatch message and no checklist', async () => {
		setupRepo({ repoResult: { code: 'identity-mismatch' } });
		await expect.element(page.getByText(/different GitHub account/)).toBeInTheDocument();
		await expect.element(steps()).not.toBeInTheDocument();
	});

	it('keeps the new repository selected and shows its URL when the project fails', async () => {
		const create = vi.fn(async (): Promise<CreateProjectsOutcome> => ({
			ok: true,
			results: [
				{
					name: 'tools',
					status: 'failed',
					repositories: ['github:acme/web'],
					reason: 'cluster said no'
				}
			]
		}));
		setupRepo({ newRepo: 'github:acme/web', repoResult: { code: 'created' } }, create);
		await page.getByRole('textbox', { name: 'Project name' }).fill('tools');
		await page.getByRole('button', { name: 'Create project' }).click();
		await expect
			.element(page.getByText('The repository was created, the project was not'))
			.toBeInTheDocument();
		await expect
			.element(page.getByRole('link', { name: 'acme/web' }))
			.toHaveAttribute('href', 'https://github.com/acme/web');
		await expect.element(page.getByRole('button', { name: 'Remove acme/web' })).toBeInTheDocument();
		await expect
			.element(steps().getByRole('listitem').nth(2))
			.toHaveAttribute('data-state', 'failed');
	});

	it('hides the section when no starter is given', async () => {
		setup();
		await expect
			.element(page.getByRole('button', { name: 'Create repository on GitHub' }))
			.not.toBeInTheDocument();
	});
});
