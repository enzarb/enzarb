<script lang="ts" module>
	export interface SetupSandbox {
		name: string;
		displayName: string;
		ready: boolean;
	}
</script>

<script lang="ts">
	import { claudeCredential } from '#lib/claude.remote.ts';
	import { clearClaudeToken } from '#lib/credential-clear.ts';
	import { terminalConnection } from '#lib/terminal.remote.ts';
	import ClaudeTokenStatus from './ClaudeTokenStatus.svelte';
	import CredentialCaptured from './CredentialCaptured.svelte';
	import Terminal from './Terminal.svelte';
	import Button from './form/Button.svelte';
	import Select from './form/Select.svelte';
	import Notice from './ui/Notice.svelte';

	/**
	 * The user's Claude token: status from `app_api.claude_credential_v`, guided `claude setup-token`
	 * in one of the user's sandboxes, and Remove (`credential_clear`). The token never reaches the UI.
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
			const proto = await import('@enzarb/proto');
			const gone = await clearClaudeToken(sandbox, {
				connect: async (name) => {
					const { url } = await terminalConnection(name);
					return proto.Session.connect(url, { client: 'ui' });
				},
				stillConfigured: async () => {
					await credential.refresh();
					return credential.current?.configured ?? false;
				}
			});
			if (!gone) removeError = 'The token was not removed yet. Reload the page to check again.';
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
		<div class="ez-cluster">
			{#if mode !== 'setup'}
				<Button variant="primary" disabled={removing} onclick={() => (mode = 'setup')}>
					{configured ? 'Replace token' : 'Set up'}
				</Button>
			{/if}
			{#if configured}
				<Button variant="danger" loading={removing} onclick={remove}>Remove</Button>
			{/if}
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
