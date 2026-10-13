<script lang="ts">
	import type { AboutRelease } from '#lib/about-types.ts';
	import { isGithubUrl } from '#lib/versions.ts';

	/** Releases newest first; the one at `openIndex` (the deployed release) starts expanded. */
	let { releases, openIndex }: { releases: AboutRelease[]; openIndex: number } = $props();
</script>

<ul class="ez-release-list">
	{#each releases as release, i (release.tag)}
		<li>
			<details class="ez-release" open={i === openIndex}>
				<summary>
					<span class="ez-release__name">{release.name}</span>
					{#if release.publishedAt}
						<time class="ez-muted ez-text-sm" datetime={release.publishedAt}
							>{release.publishedAt.slice(0, 10)}</time
						>
					{/if}
				</summary>
				<!-- eslint-disable-next-line svelte/no-at-html-tags -- sanitized by renderMarkdown on the server -->
				<div class="ez-release__body">{@html release.html}</div>
				{#if isGithubUrl(release.url)}
					<a href={release.url} target="_blank" rel="noopener noreferrer">View release on GitHub</a>
				{:else if release.url}
					<span class="ez-muted">View release on GitHub</span>
				{/if}
			</details>
		</li>
	{/each}
</ul>
