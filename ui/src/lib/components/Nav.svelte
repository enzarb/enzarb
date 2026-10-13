<script lang="ts">
	import { page } from '$app/state';

	/** Items waiting on the user; shown as a badge on the Work link when above zero. */
	let { needsYou = 0 }: { needsYou?: number } = $props();

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
				: undefined}
			>{item.label}{#if item.href === '/work' && needsYou > 0}<span
					class="ez-badge"
					role="img"
					aria-label="{needsYou} {needsYou === 1 ? 'item needs' : 'items need'} you"
					>{needsYou}</span
				>{/if}</a
		>
	{/each}
</nav>
