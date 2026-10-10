<script lang="ts" module>
	import type { IconName } from './Icon.svelte';

	export type StepStatus = 'pending' | 'active' | 'done' | 'failed';

	export interface StepItem {
		id: string | number;
		status: StepStatus;
		label: string;
		/** Muted text before the label (e.g. the previous state in a timeline). */
		prefix?: string;
		/** Right-hand status word ("Done", "In progress"). */
		stateText?: string;
		/** A timestamp shown instead of `stateText`. */
		time?: { iso: string; text: string };
		/** Detail line (failure reason, event type). */
		message?: string;
		/** Overrides the glyph for the status. */
		mark?: IconName;
		/** Extra attributes on the <li> (e.g. `data-state`). */
		attrs?: Record<string, string>;
	}
</script>

<script lang="ts">
	import Icon from './Icon.svelte';

	/**
	 * The step checklist: the way every long operation shows progress (boot, terminal connection,
	 * credential check) and how a work item's history reads. Fixed glyphs per state, never an
	 * open-ended spinner. `label` names the list for assistive tech.
	 */
	let { items, label, ...rest }: { items: StepItem[]; label: string; 'data-state'?: string } =
		$props();

	const marks: Record<StepStatus, IconName> = {
		pending: 'circle',
		active: 'progress',
		done: 'check',
		failed: 'x'
	};
</script>

<ol class="ez-steps" aria-label={label} {...rest}>
	{#each items as s (s.id)}
		<li class="ez-steps__item ez-steps__item--{s.status}" {...s.attrs}>
			<span class="ez-steps__mark" aria-hidden="true"
				><Icon name={s.mark ?? marks[s.status]} /></span
			>
			<span class="ez-steps__label">
				{#if s.prefix}<span class="ez-muted">{s.prefix}</span>{/if}
				{s.label}
			</span>
			{#if s.time}
				<time class="ez-steps__state" datetime={s.time.iso}>{s.time.text}</time>
			{:else if s.stateText}
				<span class="ez-steps__state">{s.stateText}</span>
			{/if}
			{#if s.message}<span class="ez-steps__message">{s.message}</span>{/if}
		</li>
	{/each}
</ol>
