<script lang="ts">
	import { formatDuration } from '#lib/duration.ts';
	import type { MetricsRow } from '#lib/tasks.ts';
	import { formatCost, formatTokens } from '#lib/usage.ts';

	/** `engine_api.metrics` rows: one per workflow, phase and step. */
	let { rows }: { rows: MetricsRow[] } = $props();
</script>

<div class="ez-table-wrap">
	<table class="ez-table">
		<thead>
			<tr>
				<th scope="col">Workflow</th>
				<th scope="col">Phase</th>
				<th scope="col">Step</th>
				<th scope="col">Runs</th>
				<th scope="col">Avg duration</th>
				<th scope="col">Active</th>
				<th scope="col">Queued</th>
				<th scope="col">Tokens in / out</th>
				<th scope="col">Cost</th>
				<th scope="col">Retries</th>
				<th scope="col">Resumes</th>
				<th scope="col">Stalls</th>
				<th scope="col">Steers</th>
			</tr>
		</thead>
		<tbody>
			{#each rows as r (`${r.workflow}|${r.phase}|${r.step}`)}
				<tr>
					<td>{r.workflow ?? '—'}</td>
					<td>{r.phase}</td>
					<td>{r.step}</td>
					<td>{r.runs}</td>
					<td>{formatDuration(r.avgWallSecs)}</td>
					<td>{formatDuration(r.activeSecs)}</td>
					<td>{formatDuration(r.queueSecs)}</td>
					<td>{formatTokens(r.inputTokens)} / {formatTokens(r.outputTokens)}</td>
					<td>{formatCost(r.costUsd)}</td>
					<td>{r.retries}</td>
					<td>{r.resumes}</td>
					<td>{r.stalls}</td>
					<td>{r.steers}</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
