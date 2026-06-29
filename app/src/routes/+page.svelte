<script lang="ts">
	import { getOptionalSession } from '$lib/remote/session.remote';
	import { getPublicPricing } from '$lib/remote/billing.remote';

	const usd = (n: number, decimals = 4) =>
		'$' + n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: decimals });
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
				<div class="feature-icon">📦</div>
				<h3>Integrated registry</h3>
				<p>Every project gets a private OCI container registry. No external accounts needed — everything lives in-cluster.</p>
			</div>
			<div class="feature-card">
				<div class="feature-icon">🚀</div>
				<h3>Deploy anywhere</h3>
				<p>Custom domains, automatic TLS, and one-click deployments. Go from code to production without leaving your workspace.</p>
			</div>
		</div>
	</section>

	{#await getPublicPricing() then p}
	<section class="pricing">
		<div class="pricing-inner">
			<h2>Simple, usage-based pricing</h2>
			<p class="pricing-sub">Pay only for what you use. Start free — no credit card required.</p>
			<div class="pricing-tiers">
				<div class="tier-card tier-free">
					<div class="tier-name">Free</div>
					<div class="tier-price">$0 <span class="tier-period">/ month</span></div>
					<ul class="tier-features">
						<li>{p.freeVCPUHours.toLocaleString()} vCPU-hours compute</li>
						<li>{p.freeMemGiBHours.toLocaleString()} GiB-hours memory</li>
						<li>{p.freeBlockStorageGiBMonths} GiB-months block storage</li>
						<li>{p.freeRegistryGiBMonths} GiB-months registry storage</li>
						<li>{p.freeNetEgressExternalGib} GiB external egress</li>
					</ul>
				</div>
				<div class="tier-card tier-payg">
					<div class="tier-name">Pay-as-you-go</div>
					<div class="tier-price">Usage <span class="tier-period">based</span></div>
					<table class="rate-table">
						<tbody>
							<tr><td>Compute</td><td class="rate">{usd(p.vcpuHoursPerUnit, 4)} / vCPU-hr</td></tr>
							<tr><td>Memory</td><td class="rate">{usd(p.memGiBHoursPerUnit, 4)} / GiB-hr</td></tr>
							<tr><td>Block storage</td><td class="rate">{usd(p.blockStorageGiBMonthsPerUnit, 4)} / GiB-mo</td></tr>
							<tr><td>Registry storage</td><td class="rate">{usd(p.registryGiBMonthsPerUnit, 4)} / GiB-mo</td></tr>
							<tr><td>Egress</td><td class="rate">{usd(p.netEgressExternalPerGib, 4)} / GiB</td></tr>
						</tbody>
					</table>
				</div>
			</div>
		</div>
	</section>
	{/await}

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

	/* Pricing section */
	.pricing {
		background: var(--color-bg);
		border-top: 1px solid var(--color-border);
		padding: 3.5rem 2rem;
	}
	.pricing-inner {
		max-width: 760px;
		margin: 0 auto;
	}
	.pricing h2 {
		font-size: 1.5rem;
		font-weight: 700;
		margin: 0 0 0.5rem;
		text-align: center;
	}
	.pricing-sub {
		color: var(--color-text-muted);
		font-size: 0.9375rem;
		text-align: center;
		margin: 0 0 2.5rem;
	}
	.pricing-tiers {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1.25rem;
	}
	.tier-card {
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: 8px;
		padding: 1.5rem;
	}
	.tier-free { border-color: var(--color-accent); }
	.tier-name {
		font-size: 11px;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--color-text-muted);
		margin-bottom: 0.5rem;
	}
	.tier-price {
		font-size: 1.75rem;
		font-weight: 700;
		margin-bottom: 1.25rem;
		color: var(--color-text);
	}
	.tier-period { font-size: 0.875rem; font-weight: 400; color: var(--color-text-muted); }
	.tier-features {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}
	.tier-features li {
		font-size: 0.875rem;
		color: var(--color-text-muted);
		padding-left: 1.25rem;
		position: relative;
	}
	.tier-features li::before {
		content: '✓';
		position: absolute;
		left: 0;
		color: var(--color-accent);
		font-size: 12px;
	}
	.rate-table { width: 100%; border-collapse: collapse; }
	.rate-table td { padding: 0.35rem 0; font-size: 0.875rem; vertical-align: middle; }
	.rate-table td:first-child { color: var(--color-text-muted); }
	.rate-table .rate { text-align: right; font-variant-numeric: tabular-nums; font-family: var(--font-mono); font-size: 12px; }
	.rate-table tr + tr td { border-top: 1px solid var(--color-border); }

	@media (max-width: 560px) {
		.pricing-tiers { grid-template-columns: 1fr; }
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
