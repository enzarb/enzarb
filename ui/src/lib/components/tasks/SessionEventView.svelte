<script lang="ts">
	import { eventText, isError, toolBody, toolName, usageOf } from '#lib/session.ts';
	import type { SessionEvent } from '#lib/tasks.ts';
	import { formatCost, formatTokens } from '#lib/usage.ts';

	/** One captured session event as a conversation turn. Tool calls and results collapse. */
	let { event }: { event: SessionEvent } = $props();

	const LABELS: Record<string, string> = {
		prompt: 'Prompt',
		user: 'User',
		steer: 'Steer',
		assistant: 'Assistant',
		thinking: 'Thinking',
		system: 'System',
		stdout: 'stdout',
		stderr: 'stderr',
		tool_call: 'Tool call',
		tool_result: 'Tool result',
		usage: 'Usage'
	};
	const text = $derived(eventText(event));
	const error = $derived(isError(event));
</script>

<div
	class="ez-session-event ez-session-event--{event.kind}"
	class:ez-session-event--error={error}
	data-kind={event.kind}
	data-seq={event.seq}
	id="seq-{event.seq}"
>
	<div class="ez-session-event__meta">
		<span class="ez-session-event__kind">{LABELS[event.kind] ?? event.kind}</span>
		<time class="ez-muted" datetime={event.at}>{new Date(event.at).toLocaleTimeString()}</time>
		{#if event.truncated}<span class="ez-badge ez-badge--warning" title="Middle of payload dropped"
				>truncated</span
			>{/if}
		{#if error}<span class="ez-badge ez-badge--danger">error</span>{/if}
	</div>
	{#if event.kind === 'tool_call' || event.kind === 'tool_result'}
		<details>
			<summary><code>{toolName(event)}</code></summary>
			<pre class="ez-session-event__pre">{toolBody(event)}</pre>
		</details>
	{:else if event.kind === 'thinking'}
		<details>
			<summary class="ez-muted">Thinking…</summary>
			<p class="ez-session-event__text">{text ?? ''}</p>
		</details>
	{:else if event.kind === 'usage'}
		{@const u = usageOf(event)}
		<p class="ez-muted">
			{formatTokens(u.input)} in · {formatTokens(u.output)} out{#if u.cost !== null}
				· {formatCost(u.cost)}{/if}
		</p>
	{:else if event.kind === 'stdout' || event.kind === 'stderr'}
		<pre class="ez-session-event__pre">{text ?? JSON.stringify(event.payload)}</pre>
	{:else}
		<p class="ez-session-event__text">{text ?? JSON.stringify(event.payload)}</p>
	{/if}
</div>
