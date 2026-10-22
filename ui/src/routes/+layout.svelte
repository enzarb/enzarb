<script lang="ts">
	import favicon from '#lib/assets/favicon-ez.svg';
	import '@fontsource/jost/latin-700.css';
	import '#lib/styles/global.scss';
	import ToastRegion from '#lib/components/ui/ToastRegion.svelte';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
</svelte:head>

{@render children()}
<ToastRegion />
