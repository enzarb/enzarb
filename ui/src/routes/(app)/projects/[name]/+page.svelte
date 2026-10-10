<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import Button from '#lib/components/form/Button.svelte';
	import ConditionChecklist from '#lib/components/ConditionChecklist.svelte';
	import CredentialStatus from '#lib/components/CredentialStatus.svelte';
	import ConfirmDialog from '#lib/components/ui/ConfirmDialog.svelte';
	import KeyValueList from '#lib/components/ui/KeyValueList.svelte';
	import LinkButton from '#lib/components/ui/LinkButton.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { deleteProject, getProject } from '#lib/projects.remote.ts';
	import { getProjectSettings } from '#lib/terminal.remote.ts';

	const POLL_MS = 2000;
	const project = $derived(getProject(page.params.name!));
	const settings = $derived(getProjectSettings(page.params.name!));

	// Poll while the project is not Ready (no timers once it is).
	$effect(() => {
		if (project.current?.ready) return;
		const id = setInterval(() => void project.refresh(), POLL_MS);
		return () => clearInterval(id);
	});

	// The sandbox's own answer, once it arrives (overrides the cached flag).
	let configured = $state<boolean | undefined>();
	// A different project has its own answer.
	$effect(() => {
		void page.params.name;
		configured = undefined;
	});

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

<svelte:head><title>{page.params.name} · enzarb</title></svelte:head>

<svelte:boundary>
	{#if project.current}
		{@const p = project.current}
		<PageHeader title={p.displayName}>
			{#snippet meta()}
				<code>{p.name}</code>
				{#if p.sandboxName}<span class="ez-muted">sandbox <code>{p.sandboxName}</code></span>{/if}
			{/snippet}
			{#snippet actions()}
				<Button variant="danger" onclick={() => (confirming = true)}>Delete project</Button>
			{/snippet}
		</PageHeader>
		{#if deleteError}<Notice tone="error">{deleteError}</Notice>{/if}

		<Section title="Boot progress">
			<ConditionChecklist conditions={p.conditions} />
		</Section>

		<Section title="Sandbox">
			<CredentialStatus
				project={p.name}
				cached={settings.current?.claudeTokenConfigured ?? false}
				onchecked={(c) => (configured = c)}
			/>
			{#if settings.current?.claudeTokenConfigured && settings.current.claudeConfiguredBy}
				<p class="ez-muted">Configured by {settings.current.claudeConfiguredBy}</p>
			{/if}
			<div class="ez-cluster">
				<LinkButton href="/projects/{p.name}/terminal">Open terminal</LinkButton>
				<LinkButton href="/projects/{p.name}/claude">
					{(configured ?? settings.current?.claudeTokenConfigured) ? 'Redo' : 'Start'} Claude setup
				</LinkButton>
			</div>
		</Section>

		<Section title="Repositories">
			{#each p.repositories as r (r.url)}
				<p><a href={r.url}>{r.url}</a> <span class="ez-muted">({r.defaultBranch})</span></p>
			{:else}
				<p class="ez-muted">No repositories.</p>
			{/each}
		</Section>

		<Section title="Triggers">
			<KeyValueList
				items={[
					{
						term: 'Issues',
						value: p.triggers.issues ? 'On' : 'Off',
						tone: p.triggers.issues ? 'success' : 'neutral'
					},
					{
						term: 'Change requests',
						value: p.triggers.changeRequests ? 'On' : 'Off',
						tone: p.triggers.changeRequests ? 'success' : 'neutral'
					},
					{
						term: 'Mentions',
						value: p.triggers.mentions ? 'On' : 'Off',
						tone: p.triggers.mentions ? 'success' : 'neutral'
					}
				]}
			/>
		</Section>

		<ConfirmDialog
			bind:open={confirming}
			title="Delete {p.displayName}?"
			description="The sandbox and everything in it are removed. This cannot be undone."
			confirmLabel="Confirm delete"
			onconfirm={remove}
		/>
	{:else}
		<Skeleton label="Loading project" rows={4} />
	{/if}
	{#snippet failed()}
		<PageHeader title="Project not found" />
		<div><LinkButton href="/projects">Back to projects</LinkButton></div>
	{/snippet}
</svelte:boundary>
