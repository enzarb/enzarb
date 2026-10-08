<script lang="ts">
	import { page } from '$app/state';

	const items = [
		{ href: '/projects', label: 'Projects' },
		{ href: '/work', label: 'Work items' }
	];
</script>

<nav class="nav" aria-label="Main">
	{#each items as item (item.href)}
		<a
			href={item.href}
			aria-current={page.url.pathname === item.href || page.url.pathname.startsWith(item.href + '/')
				? 'page'
				: undefined}>{item.label}</a
		>
	{/each}
</nav>
