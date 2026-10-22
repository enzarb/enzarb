<script lang="ts">
	import Button from '../form/Button.svelte';
	import Badge from '../ui/Badge.svelte';
	import { diagnosticTarget, type Diagnostic, type WorkflowDoc } from '#lib/workflows-types.ts';

	/** The workflows in file order, the selected one marked. */
	let {
		workflows,
		selected = $bindable(0),
		diagnostics,
		add
	}: {
		workflows: WorkflowDoc[];
		selected?: number;
		diagnostics: readonly Diagnostic[];
		add: (example: boolean) => void;
	} = $props();

	const errorsIn = (id: string) =>
		diagnostics.filter((d) => d.severity === 'error' && diagnosticTarget(d.path)?.workflow === id)
			.length;
</script>

<div class="ez-stack">
	<ul class="ez-modes-list" aria-label="Workflows">
		{#each workflows as w, i (i)}
			{@const n = errorsIn(w.id)}
			<li>
				<button
					type="button"
					class="ez-modes-list__item"
					aria-current={i === selected ? 'true' : undefined}
					onclick={() => (selected = i)}
				>
					<span class="ez-mono">{w.id || '(no id)'}</span>
					<span class="ez-muted">{w.on.join(', ')}</span>
					{#if n}<Badge tone="danger">{n} {n === 1 ? 'error' : 'errors'}</Badge>{/if}
				</button>
			</li>
		{/each}
	</ul>
	<div class="ez-cluster">
		<Button size="sm" onclick={() => add(false)}>Add workflow</Button>
		<Button size="sm" onclick={() => add(true)}>Add the example</Button>
	</div>
</div>
