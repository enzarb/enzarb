<script lang="ts">
	import Badge from '../ui/Badge.svelte';
	import { issueModeIndex, type ModeIssue } from '#lib/modes-types.ts';

	/** Every validation finding; a finding inside a mode selects that mode. */
	let { issues, select }: { issues: readonly ModeIssue[]; select: (index: number) => void } =
		$props();

	const tone = { error: 'danger', warning: 'warning', info: 'info' } as const;
</script>

{#if issues.length}
	<ul class="ez-modes-issues" aria-label="Validation">
		{#each issues as issue, n (n)}
			{@const i = issueModeIndex(issue.path)}
			<li>
				<Badge tone={tone[issue.severity] ?? 'neutral'}>{issue.severity}</Badge>
				{#if i !== null}
					<button type="button" class="ez-link-button ez-mono" onclick={() => select(i)}
						>{issue.path}</button
					>
				{:else if issue.path}
					<span class="ez-mono">{issue.path}</span>
				{/if}
				<span>{issue.message}</span>
			</li>
		{/each}
	</ul>
{:else}
	<p class="ez-muted">No problems found.</p>
{/if}
