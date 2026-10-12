<script lang="ts">
	import ConditionChecklist from '#lib/components/ConditionChecklist.svelte';
	import KeyValueList from '#lib/components/ui/KeyValueList.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import { isTerminalFailure } from '#lib/conditions.ts';
	import { getProjectContext } from '#lib/project-context.svelte.ts';

	const ctx = getProjectContext();
	const p = $derived(ctx.project);
</script>

<div class="ez-stack">
	{#if !p.ready || isTerminalFailure(p.conditions)}
		<Section title="Boot progress">
			<ConditionChecklist conditions={p.conditions} />
		</Section>
	{/if}

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
</div>
