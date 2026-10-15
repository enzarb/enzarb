<script lang="ts">
	import { formatDuration } from '#lib/duration.ts';
	import type { StepTiming } from '#lib/server/metrics-service.ts';
	import EmptyState from './ui/EmptyState.svelte';

	/**
	 * Time per workflow step. Active time (finished visits to a state) and waiting time (items
	 * waiting in a state right now) are separate tables; bars show the median, a marker the p95,
	 * both scaled to the slowest p95 or max in their table. Jobs are a small secondary table.
	 */
	let { timing }: { timing: StepTiming } = $props();

	const active = $derived(timing.states.filter((s) => s.active));
	const waiting = $derived(timing.states.filter((s) => s.waiting));
	const openExcluded = $derived(timing.states.reduce((n, s) => n + s.openExcluded, 0));
	const empty = $derived(active.length === 0 && waiting.length === 0 && timing.jobs.length === 0);

	const top = $derived(
		Math.max(0, ...active.map((s) => Math.max(s.active!.p95 ?? 0, s.active!.max ?? 0)))
	);
	const waitMean = (w: { count: number; totalS: number }) => w.totalS / w.count;
	const waitTop = $derived(Math.max(0, ...waiting.map((s) => waitMean(s.waiting!))));
	const pct = (v: number | null, max: number) =>
		v === null || max <= 0 ? 0 : Math.min(100, (v / max) * 100);
</script>

{#snippet bar(median: number | null, p95: number | null, max: number)}
	<div class="step-timing__track" aria-hidden="true">
		<div class="step-timing__bar" data-testid="bar" style:--pct={pct(median, max)}></div>
		{#if p95 !== null}
			<div class="step-timing__p95" data-testid="p95" style:--pct={pct(p95, max)}></div>
		{/if}
	</div>
{/snippet}

{#if empty}
	<EmptyState
		title="No step timing in this window"
		description="No workflow transitions happened in this time range."
		level={3}
	/>
{:else}
	<div class="step-timing" data-testid="step-timing">
		{#if active.length > 0}
			<section class="step-timing__group" aria-labelledby="step-timing-active">
				<h3 id="step-timing-active" class="step-timing__title">Active time per state</h3>
				<div class="ez-table-wrap">
					<table class="ez-table" aria-label="Active time per state">
						<thead>
							<tr>
								<th scope="col">State</th>
								<th scope="col" class="ez-table__num">Count</th>
								<th scope="col" class="ez-table__num">Min</th>
								<th scope="col" class="ez-table__num">Max</th>
								<th scope="col" class="ez-table__num">Mean</th>
								<th scope="col" class="ez-table__num">Median</th>
								<th scope="col" class="ez-table__num">p95</th>
								<th scope="col"><span class="ez-sr-only">Median and p95</span></th>
							</tr>
						</thead>
						<tbody>
							{#each active as s (s.name)}
								{@const a = s.active!}
								<tr>
									<th scope="row">{s.name}</th>
									<td class="ez-table__num">{a.n}</td>
									<td class="ez-table__num">{formatDuration(a.min)}</td>
									<td class="ez-table__num">{formatDuration(a.max)}</td>
									<td class="ez-table__num">{formatDuration(a.mean)}</td>
									<td class="ez-table__num">{formatDuration(a.median)}</td>
									<td class="ez-table__num">{formatDuration(a.p95)}</td>
									<td>{@render bar(a.median, a.p95, top)}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
				{#if openExcluded > 0}
					<p class="ez-muted">
						{openExcluded} visit{openExcluded === 1 ? '' : 's'} still open, not counted.
					</p>
				{/if}
			</section>
		{/if}

		{#if waiting.length > 0}
			<section class="step-timing__group" aria-labelledby="step-timing-waiting">
				<h3 id="step-timing-waiting" class="step-timing__title">Waiting now</h3>
				<div class="ez-table-wrap">
					<table class="ez-table" aria-label="Waiting time per state">
						<thead>
							<tr>
								<th scope="col">State</th>
								<th scope="col" class="ez-table__num">Waiting</th>
								<th scope="col" class="ez-table__num">Total</th>
								<th scope="col" class="ez-table__num">Mean</th>
								<th scope="col"><span class="ez-sr-only">Mean wait</span></th>
							</tr>
						</thead>
						<tbody>
							{#each waiting as s (s.name)}
								{@const w = s.waiting!}
								<tr>
									<th scope="row">{s.name}</th>
									<td class="ez-table__num">{w.count}</td>
									<td class="ez-table__num">{formatDuration(w.totalS)}</td>
									<td class="ez-table__num">{formatDuration(waitMean(w))}</td>
									<td>{@render bar(waitMean(w), null, waitTop)}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
				<p class="ez-muted">
					Only waits in progress are listed; waits that ended are part of active time.
				</p>
			</section>
		{/if}

		{#if timing.jobs.length > 0}
			<section class="step-timing__group" aria-labelledby="step-timing-jobs">
				<h3 id="step-timing-jobs" class="step-timing__title">Jobs by kind</h3>
				<div class="ez-table-wrap">
					<table class="ez-table step-timing__jobs" aria-label="Job duration by kind">
						<thead>
							<tr>
								<th scope="col">Kind</th>
								<th scope="col" class="ez-table__num">Count</th>
								<th scope="col" class="ez-table__num">Min</th>
								<th scope="col" class="ez-table__num">Max</th>
								<th scope="col" class="ez-table__num">Mean</th>
								<th scope="col" class="ez-table__num">Median</th>
								<th scope="col" class="ez-table__num">p95</th>
							</tr>
						</thead>
						<tbody>
							{#each timing.jobs as j (j.kind)}
								<tr>
									<th scope="row">{j.kind}</th>
									<td class="ez-table__num">{j.stats.n}</td>
									<td class="ez-table__num">{formatDuration(j.stats.min)}</td>
									<td class="ez-table__num">{formatDuration(j.stats.max)}</td>
									<td class="ez-table__num">{formatDuration(j.stats.mean)}</td>
									<td class="ez-table__num">{formatDuration(j.stats.median)}</td>
									<td class="ez-table__num">{formatDuration(j.stats.p95)}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			</section>
		{/if}
	</div>
{/if}
