<script lang="ts">
	import Button from '../form/Button.svelte';
	import Select from '../form/Select.svelte';
	import TextField from '../form/TextField.svelte';
	import FieldSet from '../form/FieldSet.svelte';
	import type { EdgeDoc } from '#lib/workflows-types.ts';

	/**
	 * A node's `next`: edges checked in order, the first whose condition holds is taken. The last
	 * edge should have no condition (the fallback).
	 */
	let {
		edges = $bindable([]),
		targets,
		outputs,
		issues = []
	}: {
		edges: EdgeDoc[];
		/** Node ids an edge can point at. */
		targets: string[];
		/** The node's outputs, bare inside conditions. */
		outputs: readonly string[];
		issues?: string[];
	} = $props();

	const options = $derived(targets.map((t) => ({ value: t, label: t })));

	function move(i: number, by: number) {
		const next = [...edges];
		const [e] = next.splice(i, 1);
		next.splice(i + by, 0, e);
		edges = next;
	}
</script>

<FieldSet legend="Next">
	<p class="ez-muted">
		Checked in order; the first edge whose condition holds is taken. Leave the last condition empty
		as the fallback.{#if outputs.length}
			Conditions can use <span class="ez-mono">{outputs.join(', ')}</span>.{/if}
	</p>
	{#each edges as edge, i (i)}
		<div class="ez-wf-edge-row">
			<TextField
				label="If (edge {i + 1})"
				class="ez-control ez-mono"
				placeholder={i === edges.length - 1 ? 'always' : "result.decision == 'fix'"}
				bind:value={edge.if}
			/>
			<Select label="Go to" {options} bind:value={edge.to} placeholder="Choose a node…" />
			<div class="ez-cluster">
				<Button
					size="sm"
					disabled={i === 0}
					onclick={() => move(i, -1)}
					aria-label="Move edge {i + 1} up">↑</Button
				>
				<Button
					size="sm"
					disabled={i === edges.length - 1}
					onclick={() => move(i, 1)}
					aria-label="Move edge {i + 1} down">↓</Button
				>
				<Button
					size="sm"
					variant="danger"
					onclick={() => (edges = edges.filter((_, j) => j !== i))}
					aria-label="Remove edge {i + 1}">Remove</Button
				>
			</div>
		</div>
	{/each}
	<div>
		<Button size="sm" onclick={() => (edges = [...edges, { if: '', to: '' }])}>Add edge</Button>
	</div>
	{#each issues as msg, n (n)}<p class="ez-field__error">{msg}</p>{/each}
</FieldSet>
