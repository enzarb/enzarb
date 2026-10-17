<script lang="ts">
	import { untrack } from 'svelte';
	import Button from '#lib/components/form/Button.svelte';
	import { formatDuration } from '#lib/duration.ts';
	import type { SessionEvent, StepRun } from '#lib/tasks.ts';
	import { formatCost, formatTokens } from '#lib/usage.ts';
	import SessionInspector from './SessionInspector.svelte';

	/** Step runs with their metrics; each opens its session inspector (live while it runs). */
	let {
		runs,
		load
	}: {
		runs: StepRun[];
		load: (runId: string, afterSeq: number) => Promise<SessionEvent[]>;
	} = $props();

	// A running step opens its transcript (tailing live) so progress shows without a click.
	let open = $state<string | null>(untrack(() => runs.find((r) => !r.endedAt)?.runId ?? null));
</script>

{#each runs as r (r.runId)}
	<div class="ez-step-run" data-testid="step-run">
		<div class="ez-step-run__head">
			<strong>{r.step}</strong>
			<span class="ez-muted">{r.node} · attempt {r.attempt}</span>
			<span class="ez-badge" class:ez-badge--info={!r.endedAt}
				>{r.endedAt ? (r.exit ?? 'ended') : 'running'}</span
			>
		</div>
		<dl class="ez-step-run__metrics">
			<div>
				<dt>Runtime</dt>
				<dd>{r.runtime ?? '—'}{r.model ? ` · ${r.model}` : ''}</dd>
			</div>
			<div>
				<dt>Active</dt>
				<dd>{formatDuration(r.activeSecs)}</dd>
			</div>
			<div>
				<dt>Queued</dt>
				<dd>{formatDuration(r.queueSecs)}</dd>
			</div>
			<div>
				<dt>Tokens</dt>
				<dd>
					{formatTokens(r.tokens?.input ?? null)} in · {formatTokens(r.tokens?.output ?? null)} out
				</dd>
			</div>
			<div>
				<dt>Cost</dt>
				<dd>{formatCost(r.costUsd)}</dd>
			</div>
			<div>
				<dt>Resumes / stalls / steers</dt>
				<dd>{r.resumes} / {r.stalls} / {r.steers}</dd>
			</div>
		</dl>
		<Button
			size="sm"
			variant="ghost"
			aria-expanded={open === r.runId}
			onclick={() => (open = open === r.runId ? null : r.runId)}
			>{open === r.runId ? 'Hide transcript' : 'Show transcript'}</Button
		>
		{#if open === r.runId}
			<SessionInspector load={(after) => load(r.runId, after)} live={!r.endedAt} />
		{/if}
	</div>
{:else}
	<p class="ez-muted">No steps have run yet.</p>
{/each}
