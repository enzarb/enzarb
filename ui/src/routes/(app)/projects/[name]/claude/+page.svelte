<script lang="ts">
	import { page } from '$app/state';
	import Card from '#lib/components/ui/Card.svelte';
	import CredentialCaptured from '#lib/components/CredentialCaptured.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
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
<PageHeader title="Set up Claude" />
<Card>
	<ol class="ez-stack" style="--ez-gap: var(--ez-space-2)">
		<li>Wait for the connection checklist to finish; <code>claude setup-token</code> starts.</li>
		<li>Open the URL it prints, sign in with your Claude account and approve access.</li>
		<li>Paste the code you are given back into the terminal below.</li>
	</ol>
</Card>
{#if captured}<CredentialCaptured name={page.params.name!} {saveError} />{/if}
{#key page.params.name}
	<Terminal
		project={page.params.name!}
		command={['claude', 'setup-token']}
		oncredential={onCredential}
	/>
{/key}
