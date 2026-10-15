<script lang="ts">
	import { PROJECT_TABS } from '#lib/project-context.svelte.ts';

	let { name, pathname }: { name: string; pathname: string } = $props();

	const base = $derived(`/projects/${encodeURIComponent(name)}`);
	const current = (slug: string) => {
		const path = pathname.replace(/\/$/, '');
		if (!slug) return path === base;
		return path === `${base}/${slug}` || path.startsWith(`${base}/${slug}/`);
	};
</script>

<nav class="ez-tabnav" aria-label="Project sections">
	{#each PROJECT_TABS as t (t.slug)}
		<a
			class="ez-tabnav__link"
			href={t.slug ? `${base}/${t.slug}` : base}
			aria-current={current(t.slug) ? 'page' : undefined}>{t.label}</a
		>
	{/each}
</nav>
