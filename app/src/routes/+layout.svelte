<script lang="ts">
	import favicon from '$lib/assets/favicon.svg';
	import { browser } from '$app/environment';
	import '$lib/app.css';
	let { children } = $props();

	if (browser) {
		window.onerror = (msg, src, line, col, err) => {
			fetch('/api/client-error', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ message: String(msg), stack: err?.stack, context: { src, line, col } })
			}).catch(() => {});
		};
		window.onunhandledrejection = (e) => {
			fetch('/api/client-error', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ message: String(e.reason), stack: e.reason?.stack })
			}).catch(() => {});
		};
	}
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
</svelte:head>

{@render children()}
