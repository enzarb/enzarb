<script lang="ts">
	let { data } = $props();
</script>

<div class="confirm-wrap">
	<div class="confirm-card">
		<h2>Confirm account link</h2>
		{#if !data.valid}
			<p class="error">This link request is invalid or has expired. Please try signing in with GitHub again.</p>
			<a href="/auth/login" class="btn">Back to login</a>
		{:else}
			<p>
				Your GitHub account (<strong>{data.githubEmail}</strong>) matches an existing Enzarb account.
				Sign in with your existing login method to confirm you own that account and link them together.
			</p>
			<a href={data.oidcUrl} class="btn btn-primary">Sign in to confirm link</a>
			<p class="muted">This request expires in 10 minutes. If you did not initiate this, you can safely ignore it.</p>
		{/if}
	</div>
</div>

<style>
	.confirm-wrap {
		display: flex;
		justify-content: center;
		align-items: center;
		min-height: 60vh;
		padding: 2rem;
	}
	.confirm-card {
		max-width: 460px;
		width: 100%;
		background: var(--color-bg-elevated, #fff);
		border: 1px solid var(--color-border);
		border-radius: 8px;
		padding: 2rem;
	}
	h2 { margin: 0 0 1rem; font-size: 20px; }
	p { margin: 0 0 1rem; font-size: 14px; line-height: 1.5; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.error { color: var(--color-danger); }
</style>
