<script lang="ts">
	import { timeSince } from '#lib/time-since.ts';
	import { severityClass, type TaskFinding } from '#lib/tasks.ts';

	/** The open findings on one task (`task_v.findings`). */
	let { findings, now = Date.now() }: { findings: TaskFinding[]; now?: number } = $props();
</script>

{#if findings.length}
	<ul class="ez-findings" data-testid="task-findings">
		{#each findings as f (f.key)}
			<li class="ez-finding ez-finding--{severityClass(f.severity)}">
				<div class="ez-finding__head">
					<span class="ez-finding__severity">{f.severity}</span>
					<code>{f.kind}</code>
					<span class="ez-muted">{timeSince(f.firstSeen, now)}</span>
				</div>
				<p class="ez-finding__text">{f.text}</p>
			</li>
		{/each}
	</ul>
{/if}
