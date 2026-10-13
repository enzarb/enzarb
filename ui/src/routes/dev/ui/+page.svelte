<script lang="ts">
	import Showcase from './Showcase.svelte';
	import ThemeToggle from '#lib/components/ui/ThemeToggle.svelte';
</script>

<svelte:head><title>Components · enzarb (dev)</title></svelte:head>

<div class="ez-themed">
	<header class="ez-header">
		<strong>enzarb component gallery</strong>
		<ThemeToggle />
	</header>
	<div class="ez-gallery">
		<div class="ez-gallery__panel ez-themed" data-theme="light">
			<h2 class="ez-gallery__heading">Light</h2>
			<Showcase />
		</div>
		<div class="ez-gallery__panel ez-themed" data-theme="dark">
			<h2 class="ez-gallery__heading">Dark</h2>
			<Showcase />
		</div>
	</div>
</div>
