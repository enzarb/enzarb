<script lang="ts">
	import { page } from '$app/state';
	import RunDetail from '#lib/components/RunDetail.svelte';
	import Button from '#lib/components/form/Button.svelte';
	import LinkButton from '#lib/components/ui/LinkButton.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { getRun, listRuns, stopRun } from '#lib/runs.remote.ts';
	import { answerTaskPlan, getRunPlan } from '#lib/tasks.remote.ts';
	import { isEnded } from '#lib/runs.ts';

	const name = $derived(page.params.name!);
	const id = $derived(page.params.id!);
	const run = $derived(getRun({ name, id }));
	const plan = $derived(getRunPlan({ name, id }));
	let confirmation = $state('');

	// Poll while the run is live.
	$effect(() => {
		if (!run.current || isEnded(run.current)) return;
		const t = setInterval(() => {
			void run.refresh().catch(() => {});
			void plan.refresh().catch(() => {});
		}, 2000);
		return () => clearInterval(t);
	});
</script>

<svelte:head><title>Run · {page.params.name} · enzarb</title></svelte:head>

<p><LinkButton href="/projects/{encodeURIComponent(name)}/runs">All runs</LinkButton></p>
{#if confirmation}<Notice tone="success">{confirmation}</Notice>{/if}
{#if run.current}
	<RunDetail
		run={run.current}
		plan={plan.current ?? null}
		answer={async (action, text) => {
			const r = await answerTaskPlan({ name, id, action, text }).updates(
				run,
				plan,
				listRuns({ name })
			);
			confirmation = r.ok ? r.message : '';
			return { ok: r.ok, message: r.message, runId: r.ok ? r.runId : null };
		}}
		stop={async () => {
			// The server refreshes this run and the list in the same round trip.
			await stopRun({ name, id }).updates(run, listRuns({ name }));
		}}
	/>
{:else if run.error}
	<Notice tone="error" title="Could not load the run">
		The run was not found, or enzarb-tracks could not be reached.
		{#snippet actions()}<Button onclick={() => void run.refresh()}>Retry</Button>{/snippet}
	</Notice>
{:else}
	<Skeleton label="Loading run" rows={3} />
{/if}
