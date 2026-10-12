<script lang="ts">
	import * as v from 'valibot';
	import MaxRunsInput from './MaxRunsInput.svelte';
	import Button from './form/Button.svelte';
	import Checkbox from './form/Checkbox.svelte';
	import Combobox from './form/Combobox.svelte';
	import FieldSet from './form/FieldSet.svelte';
	import RadioGroup from './form/RadioGroup.svelte';
	import TextField from './form/TextField.svelte';
	import Card from './ui/Card.svelte';
	import Notice from './ui/Notice.svelte';
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

	let {
		choices,
		refreshHref,
		create
	}: {
		choices: RepositoryChoices;
		refreshHref: string;
		create: (plans: ProjectPlan[], triggers: Triggers) => Promise<CreateProjectsOutcome>;
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

	let selected = $state<string[]>([]);
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
