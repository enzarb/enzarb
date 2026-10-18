<script lang="ts">
	import { timeSince } from '#lib/time-since.ts';
	import { findingTarget, severityClass, type FindingRow } from '#lib/tasks.ts';

	/** One open finding: severity, kind, text, age, the affected target and the filed issue. */
	let { finding, now = Date.now() }: { finding: FindingRow; now?: number } = $props();

	const target = $derived(findingTarget(finding));
</script>

<li class="ez-finding ez-finding--{severityClass(finding.severity)}">
	<div class="ez-finding__head">
		<span class="ez-finding__severity">{finding.severity}</span>
		<code>{finding.kind}</code>
		<span class="ez-muted">{timeSince(finding.firstSeen, now)}</span>
	</div>
	<p class="ez-finding__text">{finding.text}</p>
	<div class="ez-finding__meta">
		{#if target.href}<a href={target.href}>{target.label}</a>{:else}<span>{target.label}</span>{/if}
		{#if target.href}<span class="ez-muted">{finding.project}</span>{/if}
		{#if finding.issueUrl}<a href={finding.issueUrl} rel="noreferrer">Filed issue</a>{/if}
	</div>
</li>
