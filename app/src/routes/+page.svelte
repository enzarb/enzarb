<script lang="ts">
	import type { PageData } from './$types';
	let { data }: { data: PageData } = $props();
</script>

<main class="home">
	<div class="hero">
		<h1>enzarb</h1>
		<p>AI-native cloud development. Persistent workspaces, integrated git, registry, and deployments.</p>
		{#if data.session}
			<a href="/dashboard" class="btn-primary">Go to Dashboard</a>
		{:else}
			<a href="/auth/login" class="btn-primary">Sign in</a>
		{/if}
	</div>
</main>

<style>
	.home {
		display: flex;
		justify-content: center;
		align-items: center;
		min-height: 100vh;
	}
	.hero {
		text-align: center;
		max-width: 560px;
		padding: 2rem;
	}
	h1 {
		font-size: 3rem;
		font-weight: 700;
		margin: 0 0 1rem;
		letter-spacing: -0.04em;
	}
	p {
		color: var(--color-text-muted);
		margin-bottom: 2rem;
		font-size: 1.125rem;
	}
	.btn-primary {
		display: inline-block;
		padding: 0.75rem 2rem;
		background: var(--color-accent);
		color: white;
		text-decoration: none;
		border-radius: 6px;
		font-weight: 500;
	}
	.btn-primary:hover { opacity: 0.9; }
</style>
