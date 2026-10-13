<script lang="ts">
	/** Read-only list of a Project's repositories, each linked, marking ones no longer offered. */
	let {
		repos
	}: {
		repos: { key: string; label: string; url: string; missing: boolean }[];
	} = $props();
</script>

<ul class="ez-repo-list" aria-label="Repositories">
	{#each repos as r (r.key)}
		<li>
			<a href={r.url} target="_blank" rel="noopener noreferrer">{r.label}</a>
			{#if r.missing}
				<span class="ez-muted">— No longer in your GitHub list (archived or access removed)</span>
			{/if}
		</li>
	{/each}
</ul>
