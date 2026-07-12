<script lang="ts">
	import { onMount } from 'svelte';
	import TilingShell from '$lib/tiling/TilingShell.svelte';
	import { getAllOrgProjects } from '$lib/remote/projects.remote';

	// Cross-project tiling: one workspace holding panes from any project the user
	// belongs to. Mirrors the per-project tiling page but is project-agnostic —
	// each tab carries its own project ref. The `@` layout reset gives it the
	// full viewport (no app shell around it), same as the per-project route.
	let narrow = $state(false);
	let orgProjects = $state<Record<string, { slug: string; displayName: string }[]> | null>(null);

	onMount(() => {
		narrow = window.innerWidth < 768;
		const handler = () => { narrow = window.innerWidth < 768; };
		window.addEventListener('resize', handler);
		// The `@` reset skips the (app) layout load, so fetch the org/project list
		// the picker needs directly.
		getAllOrgProjects().then((v) => { orgProjects = v; }).catch(() => { orgProjects = {}; });
		return () => window.removeEventListener('resize', handler);
	});
</script>

<div class="tiling-page">
	<header class="tiling-appbar">
		<a href="/" class="back">← Home</a>
		<span class="project-name">Workspace tiling</span>
	</header>

	<div class="tiling-body">
		{#if narrow}
			<div class="mobile-message">
				<p>Tiling mode is designed for desktop browsers.</p>
				<a href="/">← Back home</a>
			</div>
		{:else if orgProjects}
			<TilingShell global orgProjects={orgProjects} />
		{:else}
			<div class="mobile-message"><p>Loading…</p></div>
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
	.tiling-body { flex: 1; overflow: hidden; display: flex; flex-direction: column; min-height: 0; }

	.mobile-message { display: flex; flex-direction: column; align-items: center; justify-content: center; height: 100%; gap: 1rem; padding: 2rem; text-align: center; }
	.mobile-message p { font-size: 15px; color: var(--color-text-muted); }
	.mobile-message a { font-size: 13px; color: var(--color-accent); }
</style>
