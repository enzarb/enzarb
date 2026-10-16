<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import Button from '#lib/components/form/Button.svelte';
	import ConfirmDialog from '#lib/components/ui/ConfirmDialog.svelte';
	import DisplayNameForm from '#lib/components/DisplayNameForm.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import RepositoriesForm from '#lib/components/RepositoriesForm.svelte';
	import SandboxForm from '#lib/components/SandboxForm.svelte';
	import SecretsSection from '#lib/components/tasks/SecretsSection.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import { getProjectContext } from '#lib/project-context.svelte.ts';
	import {
		deleteProject,
		refreshRepositoryChoices,
		repositoryChoices,
		setDisplayName,
		setMaxRuns,
		setRepositories,
		setSandboxResources
	} from '#lib/projects.remote.ts';

	const ctx = getProjectContext();
	const p = $derived(ctx.project);
	const choices = repositoryChoices();
	let confirming = $state(false);
	let deleteError = $state('');

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
	<Section title="Sandbox">
		{#key p.name}
			<SandboxForm
				project={p}
				bind:tracking={ctx.trackingResources}
				saveResources={async (input) => {
					await setSandboxResources({ name: p.name, ...input });
				}}
				saveMaxRuns={async (maxRuns) => {
					await setMaxRuns({ name: p.name, maxRuns });
				}}
			/>
		{/key}
	</Section>

	<Section title="Display name">
		<DisplayNameForm
			project={p}
			save={async (displayName) => {
				const r = await setDisplayName({ name: p.name, displayName });
				return r.ok ? { ok: true } : { ok: false, message: r.message };
			}}
		/>
	</Section>

	<Section title="Repositories">
		<RepositoriesForm
			project={p}
			choices={choices.current?.repositories ?? []}
			taken={choices.current?.taken ?? {}}
			problem={choices.current?.problem?.message ?? null}
			refresh={() => refreshRepositoryChoices()}
			save={async (keys) => {
				const r = await setRepositories({ name: p.name, repositories: keys });
				await choices.refresh();
				return r.ok ? { ok: true } : { ok: false, message: r.message };
			}}
		/>
	</Section>

	<SecretsSection project={p.name} />

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
