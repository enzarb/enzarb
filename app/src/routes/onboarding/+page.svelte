<script lang="ts">
	import { chooseUsername } from '$lib/remote/onboarding.remote';
	import { page } from '$app/state';

	const returnTo = $derived(page.url.searchParams.get('returnTo') ?? '/dashboard');
</script>

<main class="onboarding">
	<div class="card">
		<div class="logo">enzarb</div>
		<h1>Welcome to Enzarb</h1>
		<p class="subtitle">Choose your username. This will be your personal namespace — like <code>github.com/username</code>.</p>

		<form {...chooseUsername}>
			<input {...chooseUsername.fields.returnTo.as('hidden', returnTo)} />

			<label for="username">Username</label>
			<div class="input-wrap">
				<span class="prefix">enzarb.dev/</span>
				<input
					id="username"
					{...chooseUsername.fields.username.as('text')}
					placeholder="your-handle"
					autocomplete="off"
					autocapitalize="none"
					spellcheck="false"
					minlength="3"
					maxlength="39"
					required
				/>
			</div>
			<p class="hint">3–39 characters. Lowercase letters, numbers, and hyphens only. Cannot start or end with a hyphen.</p>

			{#each chooseUsername.fields.username.issues() as issue}
				<p class="error">{issue.message}</p>
			{/each}

			<button type="submit" class="btn-primary">Continue</button>
		</form>
	</div>
</main>

<style>
	.onboarding {
		display: flex;
		justify-content: center;
		align-items: center;
		min-height: 100dvh;
		padding: 1.5rem;
	}

	.card {
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: 10px;
		padding: 2.5rem;
		width: 100%;
		max-width: 440px;
	}

	.logo {
		font-size: 1.25rem;
		font-weight: 700;
		letter-spacing: -0.04em;
		color: var(--color-accent);
		margin-bottom: 1.5rem;
	}

	h1 {
		font-size: 1.5rem;
		font-weight: 700;
		margin: 0 0 0.5rem;
	}

	.subtitle {
		color: var(--color-text-muted);
		font-size: 0.875rem;
		margin: 0 0 2rem;
		line-height: 1.6;
	}

	code {
		font-family: var(--font-mono);
		background: var(--color-surface-2);
		padding: 0.1em 0.3em;
		border-radius: 3px;
		font-size: 0.8em;
	}

	label {
		display: block;
		font-weight: 500;
		font-size: 13px;
		margin-bottom: 0.5rem;
	}

	.input-wrap {
		display: flex;
		align-items: center;
		background: var(--color-surface-2);
		border: 1px solid var(--color-border);
		border-radius: var(--radius);
		overflow: hidden;
		transition: border-color 0.15s;
	}
	.input-wrap:focus-within { border-color: var(--color-accent); }

	.prefix {
		padding: 0.5rem 0.75rem;
		color: var(--color-text-muted);
		font-size: 13px;
		white-space: nowrap;
		border-right: 1px solid var(--color-border);
		user-select: none;
	}

	.input-wrap input {
		border: none;
		background: transparent;
		border-radius: 0;
		padding: 0.5rem 0.75rem;
		width: 100%;
	}
	.input-wrap input:focus { outline: none; }

	.hint {
		font-size: 12px;
		color: var(--color-text-muted);
		margin: 0.5rem 0 0;
	}

	.error {
		font-size: 13px;
		color: var(--color-danger);
		margin: 0.75rem 0 0;
		background: rgba(224, 82, 82, 0.1);
		border: 1px solid rgba(224, 82, 82, 0.3);
		border-radius: var(--radius);
		padding: 0.5rem 0.75rem;
	}

	.btn-primary {
		display: block;
		width: 100%;
		margin-top: 1.5rem;
		padding: 0.75rem;
		background: var(--color-accent);
		color: white;
		border: none;
		border-radius: var(--radius);
		font-size: 0.9375rem;
		font-weight: 600;
		cursor: pointer;
		transition: opacity 0.15s;
	}
	.btn-primary:hover { opacity: 0.85; }
</style>
