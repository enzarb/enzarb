<script lang="ts">
	import type { AboutComponent } from '#lib/about-types.ts';
	import { isGithubUrl, matchRelease, UNKNOWN_VERSION } from '#lib/versions.ts';
	import ReleaseNotes from './ReleaseNotes.svelte';

	let { component }: { component: AboutComponent } = $props();

	const known = $derived(component.deployed !== UNKNOWN_VERSION);
	const matched = $derived(
		component.releases ? matchRelease(component.deployed, component.releases) : -1
	);
</script>

<section class="ez-card ez-about-card" aria-label={component.label}>
	<header class="ez-about-card__head">
		<h2>{component.label}</h2>
		<code class="ez-about-card__version">{component.deployed}</code>
		{#if isGithubUrl(component.repoUrl)}
			<a href={component.repoUrl} target="_blank" rel="noopener noreferrer">{component.name}</a>
		{:else}
			<span class="ez-muted">{component.name}</span>
		{/if}
	</header>
	{#if component.releases === null}
		<p class="ez-muted">Release notes unavailable.</p>
	{:else if component.releases.length === 0}
		<p class="ez-muted">No releases published yet.</p>
	{:else}
		{#if known && matched === -1}
			<p class="ez-muted">No matching release for the deployed version.</p>
		{/if}
		<ReleaseNotes releases={component.releases} openIndex={matched} />
	{/if}
</section>
