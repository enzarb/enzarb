<script lang="ts">
	import { page } from '$app/state';

	const items = [
		{ href: '/projects', label: 'Projects' },
		{ href: '/work', label: 'Work items' },
		{ href: '/settings', label: 'Settings' }
	];
</script>

<nav class="ez-nav" aria-label="Main">
	{#each items as item (item.href)}
		<a
			class="ez-nav__link"
			href={item.href}
			aria-current={page.url.pathname === item.href || page.url.pathname.startsWith(item.href + '/')
				? 'page'
				: undefined}>{item.label}</a
		>
	{/each}
</nav>
