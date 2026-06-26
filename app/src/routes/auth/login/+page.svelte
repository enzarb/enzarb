<script lang="ts">
	import { page } from '$app/state';
	import { getGithubOAuthConfig } from '$lib/remote/settings.remote';

	const returnTo = $derived(page.url.searchParams.get('returnTo') ?? '/');
</script>

{#await getGithubOAuthConfig() then { enabled: githubEnabled }}
<main class="login-page">
	<div class="login-card">
		<h1>enzarb</h1>
		<p class="subtitle">Sign in to continue</p>

		<div class="providers">
			<a class="provider-btn google" href="/auth/google?returnTo={encodeURIComponent(returnTo)}">
				<svg width="18" height="18" viewBox="0 0 18 18" xmlns="http://www.w3.org/2000/svg">
					<path d="M17.64 9.2c0-.637-.057-1.251-.164-1.84H9v3.481h4.844c-.209 1.125-.843 2.078-1.796 2.716v2.259h2.908c1.702-1.567 2.684-3.875 2.684-6.615z" fill="#4285F4"/>
					<path d="M9 18c2.43 0 4.467-.806 5.956-2.18l-2.908-2.259c-.806.54-1.837.86-3.048.86-2.344 0-4.328-1.584-5.036-3.711H.957v2.332A8.997 8.997 0 0 0 9 18z" fill="#34A853"/>
					<path d="M3.964 10.71A5.41 5.41 0 0 1 3.682 9c0-.593.102-1.17.282-1.71V4.958H.957A8.996 8.996 0 0 0 0 9c0 1.452.348 2.827.957 4.042l3.007-2.332z" fill="#FBBC05"/>
					<path d="M9 3.58c1.321 0 2.508.454 3.44 1.345l2.582-2.58C13.463.891 11.426 0 9 0A8.997 8.997 0 0 0 .957 4.958L3.964 6.29C4.672 4.163 6.656 3.58 9 3.58z" fill="#EA4335"/>
				</svg>
				Continue with Google
			</a>

			{#if githubEnabled}
			<a class="provider-btn github" href="/auth/github/login?returnTo={encodeURIComponent(returnTo)}">
				<svg width="18" height="18" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg" fill="currentColor">
					<path d="M12 0C5.374 0 0 5.373 0 12c0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23A11.509 11.509 0 0 1 12 5.803c1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576C20.566 21.797 24 17.3 24 12c0-6.627-5.373-12-12-12z"/>
				</svg>
				Continue with GitHub
			</a>
			{/if}
		</div>
	</div>
</main>
{/await}

<style>
	.login-page {
		min-height: 100dvh;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--color-bg);
	}
	.login-card {
		width: 100%;
		max-width: 360px;
		padding: 2.5rem 2rem;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: 12px;
		text-align: center;
	}
	h1 {
		font-size: 2rem;
		font-weight: 700;
		margin: 0 0 0.5rem;
		letter-spacing: -0.04em;
	}
	.subtitle {
		color: var(--color-text-muted);
		font-size: 14px;
		margin: 0 0 2rem;
	}
	.providers {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
	}
	.provider-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 0.625rem;
		padding: 0.75rem 1rem;
		border-radius: 8px;
		font-size: 14px;
		font-weight: 500;
		text-decoration: none;
		transition: opacity 0.15s;
	}
	.provider-btn:hover { opacity: 0.85; text-decoration: none; }
	.google {
		background: #fff;
		color: #3c4043;
		border: 1px solid #dadce0;
	}
	.github {
		background: #24292e;
		color: #fff;
		border: 1px solid #1b1f23;
	}
</style>
