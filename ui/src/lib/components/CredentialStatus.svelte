<script lang="ts">
	import { untrack } from 'svelte';
	import {
		advanceCheck,
		buildCredentialSteps,
		failCheck,
		startCheck,
		type CredentialCheck,
		type CredentialStepState
	} from '#lib/credential-check.ts';
	import { syncClaudeCredential, terminalConnection } from '#lib/terminal.remote.ts';

	let {
		project,
		/** The cached `app.project_settings` flag, shown while checking and when the sandbox cannot answer. */
		cached,
		onchecked
	}: {
		project: string;
		cached: boolean;
		/** The sandbox answered (the cache was updated). */
		onchecked?: (configured: boolean) => void;
	} = $props();

	let check = $state<CredentialCheck>(startCheck);
	const steps = $derived(buildCredentialSteps(check));
	const marks: Record<CredentialStepState, string> = {
		pending: '○',
		'in-progress': '◐',
		done: '✓',
		failed: '✗'
	};
	const words: Record<CredentialStepState, string> = {
		pending: 'Pending',
		'in-progress': 'In progress',
		done: 'Done',
		failed: 'Failed'
	};

	// Browser only: a fresh terminal JWT, connect, one credential_status round trip, close.
	$effect(() => {
		const name = project;
		let disposed = false;
		let session: { close(): unknown } | undefined;
		check = startCheck;

		(async () => {
			try {
				const [{ url }, proto] = await Promise.all([
					// untracked: a client `command()` bumps a $state synchronously, which would re-run this effect
					untrack(() => terminalConnection(name)),
					import('@enzarb/proto')
				]);
				if (disposed) return;
				check = advanceCheck(check);
				const s = await proto.Session.connect(url, { client: 'ui' });
				session = s;
				if (disposed) return;
				check = advanceCheck(check);
				const info = await s.credentialStatus();
				if (disposed) return;
				check = { phase: 'answered', configured: info.configured };
				await syncClaudeCredential({ name, configured: info.configured });
				if (disposed) return;
				onchecked?.(info.configured);
			} catch (e) {
				if (!disposed) check = failCheck(check, e);
			} finally {
				void session?.close();
			}
		})();

		return () => {
			disposed = true;
			void session?.close();
		};
	});
</script>

<div class="stack">
	{#if check.phase === 'checking'}
		<ol class="checklist" aria-label="Credential check">
			{#each steps as step (step.id)}
				<li class="step {step.state}" data-state={step.state}>
					<span class="mark" aria-hidden="true">{marks[step.state]}</span>
					<span class="label">{step.label}</span>
					<span class="state">{words[step.state]}</span>
				</li>
			{/each}
		</ol>
	{/if}
	<p>
		Claude token:
		{#if check.phase === 'answered'}
			{#if check.configured}<span class="badge done">Configured</span>{:else}<span class="badge"
					>Not configured</span
				>{/if}
		{:else}
			{#if cached}<span class="badge done">Configured</span>{:else}<span class="badge"
					>Not configured</span
				>{/if}
			{#if check.phase === 'checking'}
				<span class="muted">(last known; checking the sandbox)</span>
			{:else}
				<span class="muted">(last known; {check.reason})</span>
			{/if}
		{/if}
	</p>
</div>
