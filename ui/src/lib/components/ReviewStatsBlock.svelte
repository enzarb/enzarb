<script lang="ts">
	import { formatAgentTime, type ReviewStats } from '#lib/work-items.ts';

	/** What a change request cost so far: review rounds, fix rounds and summed agent job time. */
	let { stats }: { stats: ReviewStats } = $props();
</script>

<dl class="ez-metrics-summary" data-testid="review-stats">
	<div>
		<dt>Review rounds</dt>
		<dd>{stats.reviewRounds}</dd>
	</div>
	<div>
		<dt>Fix rounds</dt>
		<dd>{stats.fixRounds}</dd>
	</div>
	<div>
		<dt>Agent time</dt>
		<dd>{formatAgentTime(stats.agentSecs)}</dd>
	</div>
</dl>
