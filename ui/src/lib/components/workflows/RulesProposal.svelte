<script lang="ts">
	import Button from '../form/Button.svelte';
	import SaveSteps from '../modes/SaveSteps.svelte';
	import Notice from '../ui/Notice.svelte';
	import type { StepItem } from '../ui/StepList.svelte';
	import type { RulesDiff } from '#lib/rules-diff.ts';

	/** The agent's proposed change to the rules and modes, in plain language, to accept or discard. */
	let {
		diff,
		steps,
		busy,
		accept,
		discard
	}: {
		diff: RulesDiff;
		steps: StepItem[];
		busy: boolean;
		accept: () => void;
		discard: () => void;
	} = $props();
</script>

<section class="ez-stack" aria-label="Proposed change">
	<Notice tone="info" title="The agent proposes a change">
		<ul>
			{#each diff.summary as line (line)}<li>{line}</li>{/each}
		</ul>
	</Notice>
	<div class="ez-cluster">
		<Button variant="primary" disabled={busy} onclick={accept}>Accept and commit</Button>
		<Button disabled={busy} onclick={discard}>Discard</Button>
	</div>
	<SaveSteps label="Saving the proposal" items={steps} />
</section>
