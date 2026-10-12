<script lang="ts">
	import { untrack } from 'svelte';
	import * as v from 'valibot';
	import MaxRunsInput from './MaxRunsInput.svelte';
	import Button from './form/Button.svelte';
	import Checkbox from './form/Checkbox.svelte';
	import Combobox from './form/Combobox.svelte';
	import FieldSet from './form/FieldSet.svelte';
	import RadioGroup from './form/RadioGroup.svelte';
	import Select from './form/Select.svelte';
	import TextArea from './form/TextArea.svelte';
	import TextField from './form/TextField.svelte';
	import Card from './ui/Card.svelte';
	import Notice from './ui/Notice.svelte';
	import StepList, { type StepItem } from './ui/StepList.svelte';
	import { validateDescription, validateRepoName, type RepoResult } from '#lib/repo-names.ts';
	import {
		MAX_PROJECTS_PER_REQUEST,
		expandNameTemplate,
		planProblems,
		splitRepoKey,
		slugifyName,
		type CreateProjectsOutcome,
		type ProjectPlan,
		type Triggers
	} from '#lib/onboarding.ts';
	import { DEFAULT_MAX_RUNS, MaxRunsSchema } from '#lib/projects.ts';
	import type { Option } from './form/util.ts';

	export interface RepositoryChoices {
		repositories: { repo: string; accountLogin: string; private: boolean; defaultBranch: string }[];
		/** Repository key -> name of the user's Project that already holds it. */
		taken: Record<string, string>;
		/** Names of the user's existing Projects. */
		projectNames: string[];
		problem: { code: string; message: string } | null;
	}

	export type StartCreateRepo = (input: {
		owner: string;
		name: string;
		visibility: 'private' | 'public';
		description: string;
	}) => Promise<{ ok: true; href: string } | { ok: false; message: string }>;

	let {
		choices,
		refreshHref,
		create,
		startCreateRepo,
		newRepo = null,
		repoResult = null,
		navigate = (href: string) => location.assign(href)
	}: {
		choices: RepositoryChoices;
		refreshHref: string;
		create: (plans: ProjectPlan[], triggers: Triggers) => Promise<CreateProjectsOutcome>;
		/** Starts the GitHub re-authorization that creates a repository; omit to hide the section. */
		startCreateRepo?: StartCreateRepo;
		/** Key of the repository the last create-repository round trip made. */
		newRepo?: string | null;
		/** How that round trip ended (`status` is GitHub's HTTP status, when it refused). */
		repoResult?: { code: RepoResult; status?: number } | null;
		navigate?: (href: string) => void;
	} = $props();

	const shortName = (key: string) => key.replace(/^github:/, '');

	const options = $derived<Option[]>(
		choices.repositories.map((r) => ({
			value: r.repo,
			label: shortName(r.repo),
			group: r.accountLogin,
			disabled: r.repo in choices.taken,
			description:
				r.repo in choices.taken ? `in ${choices.taken[r.repo]}` : r.private ? 'private' : ''
		}))
	);

	// The repository just created comes back preselected, next to anything the user adds.
	const validNewRepo = $derived(
		newRepo && /^github:[\w.-]+\/[\w.-]+$/.test(newRepo) ? newRepo : null
	);
	let selected = $state<string[]>(
		untrack(() =>
			newRepo && options.some((o) => o.value === newRepo && !o.disabled) ? [newRepo] : []
		)
	);
	let mode = $state('single');
	let singleName = $state('');
	let displayName = $state('');
	let template = $state('{repo}');
	let overrides = $state<Record<string, string>>({});
	let triggers = $state<Triggers>({ issues: true, changeRequests: true, mentions: true });
	let maxRuns = $state<number | null | ''>(DEFAULT_MAX_RUNS);
	let pending = $state(false);
	let failure = $state<{ message: string; stale: boolean } | null>(null);
	let rejected = $state<{ name: string; reason: string }[]>([]);

	function defaultName(key: string): string {
		const parts = splitRepoKey(key);
		return parts ? expandNameTemplate(template, parts) : slugifyName(key);
	}
	const nameOf = (key: string) => overrides[key] ?? defaultName(key);

	// Empty means "use the default"; the default itself is never written to the Project.
	const maxRunsEmpty = $derived(maxRuns === null || maxRuns === '');
	const maxRunsValid = $derived(maxRunsEmpty || v.safeParse(MaxRunsSchema, maxRuns).success);
	const maxRunsExtra = $derived<{ maxRuns?: number }>(
		maxRunsValid && !maxRunsEmpty && maxRuns !== DEFAULT_MAX_RUNS
			? { maxRuns: maxRuns as number }
			: {}
	);

	const plans = $derived<ProjectPlan[]>(
		selected.length === 0
			? []
			: mode === 'single'
				? [
						{
							name: singleName.trim(),
							displayName: displayName.trim(),
							repositories: selected,
							...maxRunsExtra
						}
					]
				: selected.map((key) => ({ name: nameOf(key), repositories: [key], ...maxRunsExtra }))
	);
	const problems = $derived(planProblems(plans, new Set(choices.projectNames)));
	const valid = $derived(plans.length > 0 && maxRunsValid && problems.every((p) => p.length === 0));
	const blocked = $derived(choices.problem !== null);

	// "Create a new GitHub repository"
	const owners = $derived([...new Set(choices.repositories.map((r) => r.accountLogin))]);
	let repoOwner = $state(untrack(() => choices.repositories[0]?.accountLogin ?? ''));
	let repoName = $state('');
	let visibility = $state('private');
	let repoDescription = $state('');
	let repoPending = $state(false);
	let repoError = $state<string | null>(null);
	// The repository name defaults to the Project name.
	const effectiveRepoName = $derived(repoName.trim() || singleName.trim());
	const repoNameIssues = $derived(
		effectiveRepoName ? [validateRepoName(effectiveRepoName)].filter((m) => m !== null) : []
	);
	const repoDescriptionIssues = $derived(
		[validateDescription(repoDescription)].filter((m) => m !== null)
	);
	const canCreateRepo = $derived(
		!!effectiveRepoName &&
			repoNameIssues.length === 0 &&
			repoDescriptionIssues.length === 0 &&
			!!repoOwner &&
			!repoPending &&
			!blocked
	);

	async function createRepo() {
		if (!startCreateRepo || !canCreateRepo) return;
		repoPending = true;
		repoError = null;
		try {
			const r = await startCreateRepo({
				owner: repoOwner,
				name: effectiveRepoName,
				visibility: visibility === 'public' ? 'public' : 'private',
				description: repoDescription
			});
			if (r.ok)
				navigate(r.href); // GitHub asks the user to authorize, then returns here
			else repoError = r.message;
		} catch {
			repoError = 'Could not start creating the repository. Try again.';
		} finally {
			repoPending = false;
		}
	}

	const REPO_MESSAGES: Record<Exclude<RepoResult, 'created' | 'identity-mismatch'>, string> = {
		'created-not-installed':
			'The repository was created, but GitHub did not let this app add it to its installation. Add it to the installation in GitHub, then use Refresh repositories.',
		'owner-not-allowed': 'That account is not one of your GitHub App installations.',
		forbidden:
			'GitHub did not allow this user or token to create repositories here: the GitHub App may lack the Administration: write permission, user tokens may not be able to do this for this account type, or you may lack the right in this organization.',
		'name-taken':
			'A repository with this name already exists. Select it in the list if you created it earlier.',
		invalid: 'GitHub rejected the repository name or settings.',
		timeout:
			'GitHub did not answer in time, so the repository may exist. Use Refresh repositories.',
		failed: 'GitHub could not create the repository.'
	};

	// Create repository -> install on repository -> create Project.
	const checklist = $derived.by<StepItem[] | null>(() => {
		if (!repoResult || repoResult.code === 'identity-mismatch') return null;
		const code = repoResult.code;
		const suffix = repoResult.status ? ` (GitHub status ${repoResult.status})` : '';
		const created = code === 'created' || code === 'created-not-installed';
		const installed = code === 'created';
		const step = (
			id: string,
			label: string,
			status: StepItem['status'],
			text: string,
			message?: string
		): StepItem => ({
			id,
			label,
			status,
			stateText: text,
			message,
			attrs: { 'data-state': status }
		});
		const failedMessage = code === 'created' ? undefined : REPO_MESSAGES[code] + suffix;
		const projectStep = failure || rejected.length ? 'failed' : pending ? 'active' : 'pending';
		return [
			step(
				'create',
				'Create repository',
				created ? 'done' : 'failed',
				created ? 'Done' : 'Failed',
				created ? undefined : failedMessage
			),
			step(
				'install',
				'Install on repository',
				installed ? 'done' : created ? 'failed' : 'pending',
				installed ? 'Done' : created ? 'Failed' : 'Pending',
				created && !installed ? failedMessage : undefined
			),
			step(
				'project',
				'Create project',
				projectStep,
				{ failed: 'Failed', active: 'In progress', pending: 'Pending', done: 'Done' }[projectStep]
			)
		];
	});

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		if (!valid || pending || blocked) return;
		pending = true;
		failure = null;
		rejected = [];
		try {
			const outcome = await create($state.snapshot(plans), $state.snapshot(triggers));
			if (!outcome.ok) failure = { message: outcome.message, stale: true };
			else
				rejected = outcome.results.flatMap((r) =>
					r.status === 'failed' ? [{ name: r.name, reason: r.reason }] : []
				);
		} catch {
			failure = { message: 'Could not create the projects. Try again.', stale: false };
		} finally {
			pending = false;
		}
	}
</script>

<Card>
	<form class="ez-stack" onsubmit={submit}>
		{#if choices.problem}
			<Notice tone="error">
				{choices.problem.message}
				<a href={refreshHref} data-sveltekit-reload>Refresh repositories</a>
			</Notice>
		{/if}

		<Combobox
			label="Repositories"
			{options}
			bind:value={selected}
			placeholder="Search your repositories…"
			emptyText="No matching repositories"
			disabled={blocked}
			help="Repositories already in a project cannot be selected."
		/>

		{#if repoResult?.code === 'identity-mismatch'}
			<Notice tone="error" title="No repository was created">
				You authorized as a different GitHub account than the one you are signed in with. Try again
				and authorize as the same account.
			</Notice>
		{/if}

		{#if checklist}
			<StepList items={checklist} label="New repository progress" />
		{/if}

		{#if startCreateRepo}
			<FieldSet legend="Create a new GitHub repository">
				<Select
					label="Owner"
					options={owners.map((o) => ({ value: o, label: o }))}
					bind:value={repoOwner}
					disabled={blocked}
				/>
				<TextField
					label="Repository name"
					placeholder={singleName.trim() || 'my-repository'}
					bind:value={repoName}
					issues={repoNameIssues}
					help="Defaults to the project name."
				/>
				<RadioGroup
					label="Visibility"
					bind:value={visibility}
					options={[
						{ value: 'private', label: 'Private' },
						{ value: 'public', label: 'Public' }
					]}
				/>
				<TextArea
					label="Description"
					bind:value={repoDescription}
					issues={repoDescriptionIssues}
					rows={2}
				/>
				{#if repoError}<Notice tone="error">{repoError}</Notice>{/if}
				<div>
					<Button
						type="button"
						onclick={createRepo}
						disabled={!canCreateRepo}
						loading={repoPending}
					>
						Create repository on GitHub
					</Button>
				</div>
				<p class="ez-muted">
					You will be asked to authorize with GitHub. The repository is created with a README and
					can then be selected above with any existing ones.
				</p>
			</FieldSet>
		{/if}

		<RadioGroup
			label="Projects"
			bind:value={mode}
			options={[
				{
					value: 'single',
					label: 'One project with all selected repositories',
					description: 'They share one sandbox.'
				},
				{
					value: 'each',
					label: 'One project per repository',
					description: 'Each repository gets its own sandbox.'
				}
			]}
		/>

		{#if mode === 'single'}
			<TextField
				label="Project name"
				placeholder="my-project"
				bind:value={singleName}
				issues={singleName || selected.length ? (problems[0] ?? []) : []}
			/>
			<TextField label="Display name" bind:value={displayName} />
		{:else}
			<TextField
				label="Name template"
				bind:value={() => template, (t) => ((template = t), (overrides = {}))}
				help="{'{repo}'} and {'{owner}'} are replaced; the result is lowercased into a valid name."
			/>
			{#each selected as key, i (key)}
				<TextField
					label="Name for {shortName(key)}"
					bind:value={() => nameOf(key), (n) => (overrides[key] = n)}
					issues={problems[i] ?? []}
				/>
			{/each}
		{/if}

		<MaxRunsInput bind:value={maxRuns} allowEmpty />

		<FieldSet legend="Triggers">
			<Checkbox label="Issues" bind:checked={triggers.issues} />
			<Checkbox label="Change requests" bind:checked={triggers.changeRequests} />
			<Checkbox label="Mentions" bind:checked={triggers.mentions} />
		</FieldSet>

		{#if failure}
			<Notice tone="error">
				{failure.message}
				{#if failure.stale}<a href={refreshHref} data-sveltekit-reload>Refresh repositories</a>{/if}
			</Notice>
		{/if}

		{#if validNewRepo && (failure || rejected.length)}
			<Notice tone="warning" title="The repository was created, the project was not">
				<a href="https://github.com/{shortName(validNewRepo)}" target="_blank" rel="noreferrer"
					>{shortName(validNewRepo)}</a
				>
				is still on GitHub and stays selected. Fix the problem above and create the project again, or
				delete the repository in GitHub yourself if you do not need it.
			</Notice>
		{/if}

		{#if plans.length > MAX_PROJECTS_PER_REQUEST}
			<Notice tone="error">At most {MAX_PROJECTS_PER_REQUEST} projects per request</Notice>
		{/if}

		{#if rejected.length}
			<Notice tone="error" title="No project was created">
				<ul>
					{#each rejected as r, i (i)}
						<li><code>{r.name}</code>: {r.reason}</li>
					{/each}
				</ul>
			</Notice>
		{/if}

		<div>
			<Button variant="primary" type="submit" disabled={!valid || pending || blocked}>
				{pending
					? 'Creating…'
					: plans.length > 1
						? `Create ${plans.length} projects`
						: 'Create project'}
			</Button>
		</div>
	</form>
</Card>
