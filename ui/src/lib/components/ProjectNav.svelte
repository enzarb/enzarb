<script lang="ts">
	import { projectTabs } from '#lib/project-context.svelte.ts';

	let {
		name,
		pathname,
		chat = false
	}: {
		name: string;
		pathname: string;
		/** Show the Chat tab (ENZARB_UI_CHAT). */ chat?: boolean;
	} = $props();

	const base = $derived(`/projects/${encodeURIComponent(name)}`);
	const current = (slug: string) => {
		const path = pathname.replace(/\/$/, '');
		return slug ? path === `${base}/${slug}` : path === base;
	};
</script>

<nav class="ez-tabnav" aria-label="Project sections">
	{#each projectTabs({ chat }) as t (t.slug)}
		<a
			class="ez-tabnav__link"
			href={t.slug ? `${base}/${t.slug}` : base}
			aria-current={current(t.slug) ? 'page' : undefined}>{t.label}</a
		>
	{/each}
</nav>
