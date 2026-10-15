<script lang="ts">
	import Logo from '#lib/components/Logo.svelte';
	import Card from '#lib/components/ui/Card.svelte';
	import LinkButton from '#lib/components/ui/LinkButton.svelte';
	import ThemeToggle from '#lib/components/ui/ThemeToggle.svelte';
</script>

<svelte:head><title>Sign in · enzarb</title></svelte:head>
<main class="ez-login">
	<div class="ez-login__card">
		<Card>
			<div class="ez-stack" style="--ez-gap: var(--ez-space-5)">
				<div class="ez-login__logo"><Logo variant="wordmark" size={32} /></div>
				<p class="ez-muted">Sandboxes, events and workflows for your code repositories.</p>
				<!-- A full navigation, not enhanced: it leaves the app for GitHub. -->
				<LinkButton variant="primary" href="/login/github" data-sveltekit-reload>
					Sign in with GitHub
				</LinkButton>
			</div>
		</Card>
		<div class="ez-login__footer"><ThemeToggle /></div>
	</div>
</main>
