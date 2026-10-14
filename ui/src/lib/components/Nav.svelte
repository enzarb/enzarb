<script lang="ts">
	import { page } from '$app/state';

	/**
	 * Items waiting on the user; shown as a badge on the Work link when above zero. `needsYouMore`
	 * means more are waiting than the count (the queue is capped), so the badge reads `N+`.
	 */
	let { needsYou = 0, needsYouMore = false }: { needsYou?: number; needsYouMore?: boolean } =
		$props();

	const items = [
		{ href: '/projects', label: 'Projects' },
		{ href: '/work', label: 'Work items' }
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
					aria-label={needsYouMore
						? `${needsYou} or more items need you`
						: `${needsYou} ${needsYou === 1 ? 'item needs' : 'items need'} you`}
					>{needsYou}{needsYouMore ? '+' : ''}</span
				>{/if}</a
		>
	{/each}
</nav>
