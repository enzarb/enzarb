<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import * as v from 'valibot';
	import Button from '#lib/components/form/Button.svelte';
	import ConfirmDialog from '#lib/components/ui/ConfirmDialog.svelte';
	import MaxRunsInput from '#lib/components/MaxRunsInput.svelte';
	import MaxRunsNotice from '#lib/components/MaxRunsNotice.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import RepositoriesForm from '#lib/components/RepositoriesForm.svelte';
	import SandboxResourcesForm from '#lib/components/SandboxResourcesForm.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import { getProjectContext } from '#lib/project-context.svelte.ts';
	import {
		deleteProject,
		repositoryChoices,
		setMaxRuns,
		setRepositories,
		setSandboxResources
	} from '#lib/projects.remote.ts';
	import { DEFAULT_MAX_RUNS, MaxRunsSchema } from '#lib/projects.ts';

	const ctx = getProjectContext();
	const p = $derived(ctx.project);
	const saved = $derived(p.maxRuns ?? DEFAULT_MAX_RUNS);
	const choices = repositoryChoices();
	let confirming = $state(false);
	let deleteError = $state('');

	// Unsaved edit of the concurrency setting; undefined while the user has not touched it.
	let draft = $state<number | null | '' | undefined>();
	let saving = $state(false);
	let saveError = $state('');
	// A different project has its own answers.
	$effect(() => {
		void page.params.name;
		draft = undefined;
		saveError = '';
	});

	const value = $derived(draft === undefined ? saved : draft);
	const valid = $derived(v.safeParse(MaxRunsSchema, value).success);

	async function saveMaxRuns(value: number) {
		saving = true;
		saveError = '';
		try {
			// The default is stored as "unset" so it follows the sandbox default.
			await setMaxRuns({
				name: page.params.name!,
				maxRuns: value === DEFAULT_MAX_RUNS ? null : value
			});
			draft = undefined;
		} catch {
			saveError = 'Could not save the setting.';
		} finally {
			saving = false;
		}
	}

	async function remove() {
		deleteError = '';
		try {
			await deleteProject(page.params.name!);
			await goto('/projects');
		} catch {
			deleteError = 'Could not delete the project.';
		}
	}
</script>

<svelte:head><title>Settings · {page.params.name} · enzarb</title></svelte:head>

{#if deleteError}<Notice tone="error">{deleteError}</Notice>{/if}
<div class="ez-stack">
	<Section title="Concurrency">
		<div class="ez-settings">
			<MaxRunsInput bind:value={() => value, (x) => (draft = x)} />
			<Button
				variant="primary"
				disabled={!valid || value === saved || saving}
				onclick={() => saveMaxRuns(value as number)}
			>
				{saving ? 'Saving…' : 'Save'}
			</Button>
		</div>
		<MaxRunsNotice />
		{#if saveError}<Notice tone="error">{saveError}</Notice>{/if}
	</Section>

	<Section title="Repositories">
		<RepositoriesForm
			project={p}
			choices={choices.current?.repositories ?? []}
			taken={choices.current?.taken ?? {}}
			problem={choices.current?.problem?.message ?? null}
			save={async (keys) => {
				const r = await setRepositories({ name: p.name, repositories: keys });
				await choices.refresh();
				return r.ok ? { ok: true } : { ok: false, message: r.message };
			}}
		/>
	</Section>

	<Section title="Sandbox resources">
		<SandboxResourcesForm
			project={p}
			bind:tracking={ctx.trackingResources}
			save={async (input) => {
				await setSandboxResources({ name: p.name, ...input });
			}}
		/>
	</Section>

	<Section title="Danger zone">
		<p class="ez-muted">Deleting the project removes the sandbox and everything in it.</p>
		<div>
			<Button variant="danger" onclick={() => (confirming = true)}>Delete project</Button>
		</div>
	</Section>
</div>

<ConfirmDialog
	bind:open={confirming}
	title="Delete {p.displayName}?"
	description="The sandbox and everything in it are removed. This cannot be undone."
	confirmLabel="Confirm delete"
	onconfirm={remove}
/>
