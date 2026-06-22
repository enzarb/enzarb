<script lang="ts">
	import type { GiteaCommit } from '$lib/gitea';

	let {
		commits,
		onselect
	}: {
		commits: GiteaCommit[];
		onselect?: (commit: GiteaCommit) => void;
	} = $props();
</script>

<div class="commit-list">
	{#each commits as c (c.sha)}
		<button class="commit-row" onclick={() => onselect?.(c)}>
			<span class="commit-sha">{c.sha.slice(0, 7)}</span>
			<span class="commit-msg">{c.commit.message.split('\n')[0]}</span>
			<span class="commit-author">{c.commit.author.name}</span>
			<span class="commit-date">{new Date(c.commit.author.date).toLocaleDateString()}</span>
		</button>
	{:else}
		<p class="empty">No commits.</p>
	{/each}
</div>

<style>
	.commit-list {
		display: flex;
		flex-direction: column;
	}
	.commit-row {
		display: grid;
		grid-template-columns: 6ch 1fr auto auto;
		gap: 0.75rem;
		align-items: baseline;
		padding: 0.4rem 0.5rem;
		border-top: 1px solid var(--color-border);
		border-left: none;
		border-right: none;
		border-bottom: none;
		text-align: left;
		background: none;
		color: var(--color-text);
		cursor: pointer;
		width: 100%;
	}
	.commit-row:hover {
		background: var(--color-surface-2);
	}
	.commit-sha {
		font-family: var(--font-mono);
		font-size: 12px;
		color: var(--color-accent);
		flex-shrink: 0;
	}
	.commit-msg {
		font-size: 13px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.commit-author {
		font-size: 12px;
		color: var(--color-text-muted);
		white-space: nowrap;
	}
	.commit-date {
		font-size: 12px;
		color: var(--color-text-muted);
		white-space: nowrap;
	}
	.empty {
		color: var(--color-text-muted);
		font-size: 13px;
		padding: 2rem 0.5rem;
		text-align: center;
	}
</style>
