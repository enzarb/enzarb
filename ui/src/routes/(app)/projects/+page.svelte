<script lang="ts">
	import PageHeading from '#lib/components/PageHeading.svelte';
	import StatusBadge from '#lib/components/StatusBadge.svelte';
	import { overallState } from '#lib/conditions.ts';
	import { listProjects } from '#lib/projects.remote.ts';

	const projects = listProjects();
</script>

<svelte:head><title>Projects · enzarb</title></svelte:head>

<PageHeading title="Projects" />
<p><a class="button primary" href="/projects/new">New project</a></p>

<svelte:boundary>
	{#if projects.error}
		<p class="field-error">Could not load projects.</p>
	{:else if projects.loading}
		<p class="muted">Loading projects…</p>
	{:else if projects.current?.length}
		<ul class="project-list">
			{#each projects.current as p (p.name)}
				<li>
					<a href="/projects/{p.name}">{p.displayName}</a>
					<code class="muted">{p.name}</code>
					<StatusBadge state={overallState(p.conditions)} />
				</li>
			{/each}
		</ul>
	{:else}
		<p class="muted">No projects yet.</p>
	{/if}
	{#snippet failed()}<p class="field-error">Could not load projects.</p>{/snippet}
</svelte:boundary>
