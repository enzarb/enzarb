<script lang="ts">
	import ConditionChecklist from './ConditionChecklist.svelte';
	import LinkButton from './ui/LinkButton.svelte';
	import Section from './ui/Section.svelte';
	import type { ProjectView } from '#lib/project-types.ts';

	/** One created Project's boot checklist. */
	let { name, project }: { name: string; project: ProjectView | undefined } = $props();
</script>

<Section title={name}>
	{#snippet actions()}
		<LinkButton size="sm" href="/projects/{name}">View project</LinkButton>
	{/snippet}
	{#if project}
		<ConditionChecklist conditions={project.conditions} />
	{:else}
		<p class="ez-muted">Waiting for the cluster to pick up the project…</p>
	{/if}
</Section>
