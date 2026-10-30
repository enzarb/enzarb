<script lang="ts">
	import { page } from '$app/state';

	/** `count`: tasks waiting on the user (the Tasks link shows it when above zero). */
	let { count = 0 }: { count?: number } = $props();

	const items = $derived([
		{ href: '/projects', label: 'Projects' },
		{ href: '/tasks', label: 'Tasks', count }
	]);
</script>

<nav class="ez-nav" aria-label="Main">
	{#each items as item (item.href)}
		<a
			class="ez-nav__link"
			href={item.href}
			aria-current={page.url.pathname === item.href || page.url.pathname.startsWith(item.href + '/')
				? 'page'
				: undefined}
			aria-label={item.count ? `${item.label}, ${item.count} waiting` : undefined}
			>{item.label}{#if item.count}<span class="ez-nav__count" aria-hidden="true">{item.count}</span
				>{/if}</a
		>
	{/each}
</nav>
