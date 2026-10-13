<script lang="ts">
	import ProjectNav from '#lib/components/ProjectNav.svelte';

	/** One-row project header: title and subtext on the left, the tabs on the right. */
	let {
		displayName,
		name,
		sandboxName,
		pathname
	}: { displayName: string; name: string; sandboxName?: string | null; pathname: string } =
		$props();
</script>

<header class="ez-project-header">
	<div class="ez-project-header__text">
		<h1>{displayName}</h1>
		<div class="ez-cluster ez-project-header__meta">
			<code>{name}</code>
			{#if sandboxName}<span class="ez-muted">sandbox <code>{sandboxName}</code></span>{/if}
		</div>
	</div>
	<ProjectNav {name} {pathname} />
</header>
