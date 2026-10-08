<script lang="ts">
	import { page } from '$app/state';
	import PageHeading from '#lib/components/PageHeading.svelte';
	import Terminal from '#lib/components/Terminal.svelte';
	import { markClaudeConfigured } from '#lib/terminal.remote.ts';

	let captured = $state(false);
	let saveError = $state('');

	async function onCredential() {
		captured = true;
		try {
			await markClaudeConfigured(page.params.name!);
		} catch {
			saveError = 'The token was captured but could not be recorded. Reload the Project page.';
		}
	}
</script>

<svelte:head><title>Claude setup · {page.params.name} · enzarb</title></svelte:head>

<PageHeading title="Set up Claude" />
<ol class="stack">
	<li>Wait for the connection checklist to finish; <code>claude setup-token</code> starts.</li>
	<li>Open the URL it prints, sign in with your Claude account and approve access.</li>
	<li>Paste the code you are given back into the terminal below.</li>
</ol>

{#if captured}
	<section class="card stack" role="status">
		<h2>Claude token configured</h2>
		<p>The sandbox stored the token. You can close the terminal.</p>
		{#if saveError}<p class="field-error">{saveError}</p>{/if}
		<p><a href="/projects/{page.params.name}">Back to the Project</a></p>
	</section>
{/if}

{#key page.params.name}
	<Terminal
		project={page.params.name!}
		command={['claude', 'setup-token']}
		oncredential={onCredential}
	/>
{/key}
