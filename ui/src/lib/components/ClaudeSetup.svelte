<script lang="ts" module>
	export interface SetupSandbox {
		name: string;
		displayName: string;
		ready: boolean;
	}
</script>

<script lang="ts">
	import { claudeCredential, removeClaudeCredential } from '#lib/claude.remote.ts';
	import ClaudeTokenStatus from './ClaudeTokenStatus.svelte';
	import CredentialCaptured from './CredentialCaptured.svelte';
	import Terminal from './Terminal.svelte';
	import Button from './form/Button.svelte';
	import Select from './form/Select.svelte';
	import Notice from './ui/Notice.svelte';

	/**
	 * The user's Claude token: status from `app_api.claude_credential_v`, guided `claude setup-token`
	 * in one of the user's sandboxes, and Remove (a server-side delete of the user's own row, no
	 * sandbox needed). The token never reaches the UI.
	 */
	let { sandboxes }: { sandboxes: SetupSandbox[] } = $props();

	const credential = $derived(claudeCredential());
	const configured = $derived(credential.current?.configured ?? false);
	const ready = $derived(sandboxes.filter((s) => s.ready));
	let chosen = $state('');
	const sandbox = $derived(ready.find((s) => s.name === chosen)?.name ?? ready[0]?.name ?? '');

	let mode = $state<'idle' | 'setup' | 'captured'>('idle');
	let removing = $state(false);
	let removeError = $state('');

	async function onCaptured() {
		mode = 'captured';
		await credential.refresh().catch(() => {});
	}

	async function remove() {
		removing = true;
		removeError = '';
		try {
			await removeClaudeCredential();
			await credential.refresh();
			mode = 'idle';
		} catch {
			removeError = 'Could not remove the token. Try again.';
		} finally {
			removing = false;
		}
	}
</script>

<div class="ez-stack">
	{#if credential.error && !credential.current}
		<Notice tone="error">Could not read your Claude token status.</Notice>
	{:else}
		<ClaudeTokenStatus {configured} updatedAt={credential.current?.updatedAt} />
	{/if}

	{#if sandboxes.length === 0}
		<p class="ez-muted">
			Setup runs in one of your sandboxes. Create a Project first, then come back here.
		</p>
	{:else if ready.length === 0}
		<p class="ez-muted">None of your sandboxes is ready. Setup is available once one is running.</p>
	{:else}
		{#if ready.length > 1}
			<Select
				label="Sandbox"
				options={ready.map((s) => ({ value: s.name, label: s.displayName }))}
				value={sandbox}
				disabled={mode === 'setup' || removing}
				onchange={(v) => (chosen = v)}
			/>
		{:else}
			<p class="ez-muted">Setup runs in the sandbox of {ready[0].displayName}.</p>
		{/if}
		{#if mode !== 'setup'}
			<div class="ez-cluster">
				<Button variant="primary" disabled={removing} onclick={() => (mode = 'setup')}>
					{configured ? 'Replace token' : 'Set up'}
				</Button>
			</div>
		{/if}
	{/if}
	{#if configured}
		<div class="ez-cluster">
			<Button variant="danger" loading={removing} onclick={remove}>Remove</Button>
		</div>
	{/if}
	{#if removeError}<Notice tone="error">{removeError}</Notice>{/if}

	{#if mode === 'captured'}<CredentialCaptured />{/if}
	{#if mode === 'setup' || mode === 'captured'}
		<ol class="ez-stack" style="--ez-gap: var(--ez-space-2)">
			<li>Wait for the connection checklist to finish; <code>claude setup-token</code> starts.</li>
			<li>Open the URL it prints, sign in with your Claude account and approve access.</li>
			<li>Paste the code you are given back into the terminal below.</li>
		</ol>
		{#key sandbox}
			<Terminal project={sandbox} command={['claude', 'setup-token']} oncredential={onCaptured} />
		{/key}
	{/if}
</div>
