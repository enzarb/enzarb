<script lang="ts">
	import { METRICS_TABS } from '#lib/project-context.svelte.ts';

	let { name, pathname, search }: { name: string; pathname: string; search: string } = $props();

	const base = $derived(`/projects/${encodeURIComponent(name)}/metrics`);
	const current = (slug: string) => {
		const path = pathname.replace(/\/$/, '');
		return path === (slug ? `${base}/${slug}` : base);
	};
</script>

<nav class="ez-tabnav" aria-label="Metrics sections">
	{#each METRICS_TABS as t (t.slug)}
		<a
			class="ez-tabnav__link"
			href={`${t.slug ? `${base}/${t.slug}` : base}${search}`}
			aria-current={current(t.slug) ? 'page' : undefined}>{t.label}</a
		>
	{/each}
</nav>
