<script lang="ts">
	import ConditionChecklist from './ConditionChecklist.svelte';
	import type { ProjectView } from '#lib/project-types.ts';

	let { name, project }: { name: string; project: ProjectView | undefined } = $props();
</script>

<section class="card stack">
	<h2><a href="/projects/{name}">{name}</a></h2>
	{#if project}
		<ConditionChecklist conditions={project.conditions} />
	{:else}
		<p class="muted">Waiting for the cluster to pick up the project…</p>
	{/if}
</section>
