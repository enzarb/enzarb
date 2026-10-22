<script lang="ts">
	import Badge from '../ui/Badge.svelte';
	import { diagnosticTarget, type Diagnostic } from '#lib/workflows-types.ts';

	/** Every finding of tracks' check; one inside a workflow selects it (and its node). */
	let {
		diagnostics,
		select
	}: {
		diagnostics: readonly Diagnostic[];
		select: (target: { workflow: string; node: string | null }) => void;
	} = $props();

	const tone = { error: 'danger', warning: 'warning' } as const;
</script>

{#if diagnostics.length}
	<ul class="ez-modes-issues" aria-label="Diagnostics">
		{#each diagnostics as d, n (n)}
			{@const t = diagnosticTarget(d.path)}
			<li>
				<Badge tone={tone[d.severity] ?? 'neutral'}>{d.severity}</Badge>
				{#if t}
					<button type="button" class="ez-link-button ez-mono" onclick={() => select(t)}
						>{d.path}</button
					>
				{:else if d.path}
					<span class="ez-mono">{d.path}</span>
				{/if}
				<span>{d.message}</span>
			</li>
		{/each}
	</ul>
{:else}
	<p class="ez-muted">No problems found.</p>
{/if}
