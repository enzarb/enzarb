<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import ConditionChecklist from '#lib/components/ConditionChecklist.svelte';
	import PageHeading from '#lib/components/PageHeading.svelte';
	import { deleteProject, getProject } from '#lib/projects.remote.ts';

	const POLL_MS = 2000;

	const project = $derived(getProject(page.params.name!));

	// Poll while the project is not Ready (no timers once it is).
	$effect(() => {
		if (project.current?.ready) return;
		const id = setInterval(() => void project.refresh(), POLL_MS);
		return () => clearInterval(id);
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
		<PageHeading title={p.displayName} />
		<p class="muted">
			<code>{p.name}</code>{#if p.namespace}
				· namespace <code>{p.namespace}</code>{/if}
		</p>

		<section class="card stack">
			<h2>Boot progress</h2>
			<ConditionChecklist conditions={p.conditions} />
		</section>

		<section class="card stack">
			<h2>Repositories</h2>
			{#each p.repositories as r (r.url)}
				<p><a href={r.url}>{r.url}</a> <span class="muted">({r.defaultBranch})</span></p>
			{:else}
				<p class="muted">No repositories.</p>
			{/each}
			<h2>Triggers</h2>
			<p class="muted">
				Issues: {p.triggers.issues ? 'on' : 'off'} · Change requests: {p.triggers.changeRequests
					? 'on'
					: 'off'} · Mentions: {p.triggers.mentions ? 'on' : 'off'}
			</p>
		</section>

		<p>
			{#if confirming}
				<button class="danger" onclick={remove}>Confirm delete</button>
				<button onclick={() => (confirming = false)}>Cancel</button>
			{:else}
				<button class="danger" onclick={() => (confirming = true)}>Delete project</button>
			{/if}
		</p>
		{#if deleteError}<p class="field-error">{deleteError}</p>{/if}
	{:else}
		<p class="muted">Loading project…</p>
	{/if}
	{#snippet failed()}
		<PageHeading title="Project not found" />
		<p><a href="/projects">Back to projects</a></p>
	{/snippet}
</svelte:boundary>
