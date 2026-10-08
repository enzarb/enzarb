<script lang="ts">
	import Logo from '#lib/components/Logo.svelte';
</script>

<svelte:head><title>Sign in · enzarb</title></svelte:head>

<main class="login">
	<div class="card stack">
		<Logo />
		<p class="muted">Sandboxes, events and workflows for your code repositories.</p>
		<!-- A full navigation, not enhanced: it leaves the app for GitHub. -->
		<a class="button primary" href="/login/github" data-sveltekit-reload>Sign in with GitHub</a>
	</div>
</main>
