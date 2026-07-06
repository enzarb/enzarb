<script lang="ts">
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import TilingShell from '$lib/tiling/TilingShell.svelte';
	import { getProject } from '$lib/remote/projects.remote';

	const namespace = $derived(page.params.namespace!);
	const project = $derived(page.params.project!);
	const base = $derived(`/${namespace}/projects/${project}`);
	const projectsList = $derived(`/${namespace}/projects`);
	const projectData = $derived(getProject(project));

	let narrow = $state(false);

	onMount(() => {
		narrow = window.innerWidth < 768;
		const handler = () => { narrow = window.innerWidth < 768; };
		window.addEventListener('resize', handler);
		return () => window.removeEventListener('resize', handler);
	});
</script>

<div class="tiling-page">
	<header class="tiling-appbar">
		<a href={projectsList} class="back">← Projects</a>
		{#await projectData then proj}
			<span class="project-name">{proj.spec.displayName}</span>
		{/await}
		<a href={base} class="standard-view" title="Standard view">Standard view</a>
	</header>

	<div class="tiling-body">
		{#if narrow}
			<div class="mobile-message">
				<p>Tiling mode is designed for desktop browsers.</p>
				<a href={base}>← Back to standard view</a>
			</div>
		{:else}
			<TilingShell {namespace} {project} />
		{/if}
	</div>
</div>

<style>
	.tiling-page { display: flex; flex-direction: column; height: 100vh; height: 100dvh; overflow: hidden; }
	.tiling-appbar {
		display: flex;
		align-items: center;
		gap: 1rem;
		height: 44px;
		flex-shrink: 0;
		padding: 0 0.75rem;
		background: var(--color-surface);
		border-bottom: 1px solid var(--color-border);
	}
	.back { font-size: 12px; color: var(--color-text-muted); flex-shrink: 0; }
	.back:hover { color: var(--color-text); }
	.project-name { font-size: 13px; font-weight: 600; color: var(--color-text); flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.standard-view { font-size: 12px; color: var(--color-text-muted); flex-shrink: 0; }
	.standard-view:hover { color: var(--color-accent); }
	.tiling-body { flex: 1; overflow: hidden; display: flex; flex-direction: column; min-height: 0; }

	.mobile-message { display: flex; flex-direction: column; align-items: center; justify-content: center; height: 100%; gap: 1rem; padding: 2rem; text-align: center; }
	.mobile-message p { font-size: 15px; color: var(--color-text-muted); }
	.mobile-message a { font-size: 13px; color: var(--color-accent); }
</style>
