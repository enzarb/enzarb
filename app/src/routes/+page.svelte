<script lang="ts">
	import { getOptionalSession } from '$lib/remote/session.remote';
</script>

<main class="home">
	<section class="hero">
		<h1>enzarb</h1>
		<p class="tagline">AI-native cloud development. Persistent workspaces, integrated git, registry, and deployments.</p>
		{#await getOptionalSession()}
			<a href="/auth/login" class="btn-primary">Get started free</a>
		{:then session}
			{#if session}
				<a href="/dashboard" class="btn-primary">Go to Dashboard</a>
			{:else}
				<a href="/auth/login" class="btn-primary">Get started free</a>
			{/if}
		{:catch}
			<a href="/auth/login" class="btn-primary">Get started free</a>
		{/await}
	</section>

	<section class="features">
		<div class="feature-grid">
			<div class="feature-card">
				<div class="feature-icon">⚡</div>
				<h3>Persistent workspaces</h3>
				<p>Kubernetes-backed dev environments that stay alive between sessions. Pick up exactly where you left off, from any device.</p>
			</div>
			<div class="feature-card">
				<div class="feature-icon">🗂</div>
				<h3>Integrated git & registry</h3>
				<p>Every project gets a private Gitea repo and OCI container registry. No external accounts needed — everything lives in-cluster.</p>
			</div>
			<div class="feature-card">
				<div class="feature-icon">🚀</div>
				<h3>Deploy anywhere</h3>
				<p>Custom domains, automatic TLS, and one-click deployments. Go from code to production without leaving your workspace.</p>
			</div>
		</div>
	</section>

	<footer class="site-footer">
		<span>© {new Date().getFullYear()} Enzarb</span>
	</footer>
</main>

<style>
	.home {
		display: flex;
		flex-direction: column;
		min-height: 100dvh;
	}

	.hero {
		display: flex;
		flex-direction: column;
		justify-content: center;
		align-items: center;
		text-align: center;
		padding: 5rem 2rem 4rem;
		padding-top: max(5rem, env(safe-area-inset-top));
	}

	h1 {
		font-size: 4rem;
		font-weight: 700;
		margin: 0 0 1rem;
		letter-spacing: -0.05em;
		background: linear-gradient(135deg, #fff 40%, var(--color-accent));
		-webkit-background-clip: text;
		-webkit-text-fill-color: transparent;
		background-clip: text;
	}

	.tagline {
		color: var(--color-text-muted);
		margin: 0 0 2.5rem;
		font-size: 1.125rem;
		max-width: 520px;
		line-height: 1.6;
	}

	.btn-primary {
		display: inline-block;
		padding: 0.875rem 2.5rem;
		background: var(--color-accent);
		color: white;
		text-decoration: none;
		border-radius: 6px;
		font-weight: 600;
		font-size: 1rem;
		transition: opacity 0.15s;
	}
	.btn-primary:hover { opacity: 0.85; text-decoration: none; }

	/* Features section */
	.features {
		background: var(--color-surface);
		border-top: 1px solid var(--color-border);
		padding: 3rem 2rem;
		flex: 1;
	}

	.feature-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
		gap: 1.5rem;
		max-width: 900px;
		margin: 0 auto;
	}

	.feature-card {
		background: var(--color-bg);
		border: 1px solid var(--color-border);
		border-radius: 8px;
		padding: 1.75rem;
	}

	.feature-icon {
		font-size: 1.75rem;
		margin-bottom: 0.75rem;
	}

	.feature-card h3 {
		font-size: 1rem;
		font-weight: 600;
		margin: 0 0 0.5rem;
		color: var(--color-text);
	}

	.feature-card p {
		color: var(--color-text-muted);
		margin: 0;
		font-size: 0.875rem;
		line-height: 1.6;
	}

	/* Footer */
	.site-footer {
		background: var(--color-surface);
		border-top: 1px solid var(--color-border);
		padding: 1.25rem 2rem;
		text-align: center;
		font-size: 12px;
		color: var(--color-text-muted);
	}

	@media (max-width: 480px) {
		h1 { font-size: 2.25rem; }
		.tagline { font-size: 0.9375rem; }
		.hero { padding: 3rem 1.25rem 2.5rem; }
		.features { padding: 2rem 1rem; }
	}
</style>
