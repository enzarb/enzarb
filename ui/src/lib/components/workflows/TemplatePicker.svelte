<script lang="ts">
	import Button from '../form/Button.svelte';
	import Checkbox from '../form/Checkbox.svelte';
	import { RULE_TEMPLATES } from '#lib/rule-templates.ts';

	/** A pick-list of rule templates; nothing is checked until the user chooses. */
	let {
		action,
		busy = false,
		choose
	}: { action: string; busy?: boolean; choose: (ids: string[]) => void } = $props();

	let picked = $state<Record<string, boolean>>({});
	const ids = $derived(RULE_TEMPLATES.filter((t) => picked[t.id]).map((t) => t.id));
</script>

<div class="ez-stack">
	<ul class="ez-rules" aria-label="Rule templates">
		{#each RULE_TEMPLATES as t (t.id)}
			<li class="ez-rules__item">
				<Checkbox
					label={t.title}
					help={t.description}
					disabled={busy}
					bind:checked={() => picked[t.id] ?? false, (v) => (picked[t.id] = v)}
				/>
			</li>
		{/each}
	</ul>
	<div class="ez-cluster">
		<Button variant="primary" disabled={!ids.length || busy} onclick={() => choose(ids)}>
			{action}
		</Button>
	</div>
</div>
