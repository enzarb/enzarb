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
	import Badge from './ui/Badge.svelte';
	import StepList, { type StepItem, type StepStatus } from './ui/StepList.svelte';

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
	const status: Record<CredentialStepState, StepStatus> = {
		pending: 'pending',
		'in-progress': 'active',
		done: 'done',
		failed: 'failed'
	};
	const words: Record<CredentialStepState, string> = {
		pending: 'Pending',
		'in-progress': 'In progress',
		done: 'Done',
		failed: 'Failed'
	};
	const items = $derived<StepItem[]>(
		steps.map((step) => ({
			id: step.id,
			status: status[step.state],
			label: step.label,
			stateText: words[step.state],
			attrs: { 'data-state': step.state }
		}))
	);

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

<div class="ez-stack">
	{#if check.phase === 'checking'}
		<StepList {items} label="Credential check" />
	{/if}
	<p class="ez-cluster">
		Claude token:
		{#if check.phase === 'answered'}
			{@render configuredBadge(check.configured)}
		{:else}
			{@render configuredBadge(cached)}
			{#if check.phase === 'checking'}
				<span class="ez-muted">(last known; checking the sandbox)</span>
			{:else}
				<span class="ez-muted">(last known; {check.reason})</span>
			{/if}
		{/if}
	</p>
</div>

{#snippet configuredBadge(configured: boolean)}
	{#if configured}<Badge tone="success">Configured</Badge>{:else}<Badge tone="neutral" hollow
			>Not configured</Badge
		>{/if}
{/snippet}
