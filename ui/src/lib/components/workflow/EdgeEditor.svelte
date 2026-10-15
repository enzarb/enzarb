<script lang="ts">
	import Button from '#lib/components/form/Button.svelte';
	import { ELSE, type Edges } from '#lib/workflow/definition.ts';
	import { exprProblem } from '#lib/workflow/expr.ts';

	/**
	 * A node's `next`: a plain target, or conditions checked in order with `else` last. Conditions
	 * can be added, removed and moved; `else` always stays at the end.
	 */
	let {
		next = $bindable(),
		targets
	}: {
		next: Edges | undefined;
		targets: string[];
	} = $props();

	const uid = $props.id();
	const mode = $derived(next?.kind ?? 'none');

	function setMode(m: string) {
		const first =
			next?.kind === 'to' ? next.to : next?.kind === 'cond' ? next.conds.at(-1)?.to : '';
		const to = first || targets[0] || '';
		if (m === 'none') next = undefined;
		else if (m === 'to') next = { kind: 'to', to };
		else
			next = {
				kind: 'cond',
				conds: [
					{ when: 'ok', to },
					{ when: ELSE, to }
				]
			};
	}

	function move(i: number, d: number) {
		if (next?.kind !== 'cond') return;
		const c = [...next.conds];
		[c[i], c[i + d]] = [c[i + d], c[i]];
		next = { kind: 'cond', conds: c };
	}

	function remove(i: number) {
		if (next?.kind !== 'cond') return;
		next = { kind: 'cond', conds: next.conds.filter((_, j) => j !== i) };
	}

	function add() {
		if (next?.kind !== 'cond') return;
		const c = [...next.conds];
		const at = c.findIndex((x) => x.when === ELSE);
		c.splice(at < 0 ? c.length : at, 0, { when: '', to: targets[0] ?? '' });
		next = { kind: 'cond', conds: c };
	}
</script>

<fieldset class="wf-edges">
	<legend>Next</legend>
	<label class="ez-field__label" for="{uid}-mode">Edges</label>
	<select
		id="{uid}-mode"
		class="ez-control"
		value={mode}
		onchange={(e) => setMode(e.currentTarget.value)}
	>
		<option value="none">None</option>
		<option value="to">Always go to</option>
		<option value="cond">Conditions</option>
	</select>

	{#if next?.kind === 'to'}
		<label class="ez-field__label" for="{uid}-to">Target</label>
		<select id="{uid}-to" class="ez-control" bind:value={next.to}>
			{#each targets as t (t)}<option value={t}>{t}</option>{/each}
		</select>
	{:else if next?.kind === 'cond'}
		<ol class="wf-edges__list" aria-label="Conditions in order">
			{#each next.conds as c, i (i)}
				{@const problem = c.when === ELSE ? null : exprProblem(c.when)}
				<li class="wf-edges__row">
					{#if c.when === ELSE && i === next.conds.length - 1}
						<span class="wf-edges__else">else</span>
					{:else}
						<input
							class="ez-control"
							aria-label="Condition {i + 1}"
							aria-invalid={problem ? 'true' : undefined}
							title={problem ?? undefined}
							bind:value={c.when}
						/>
					{/if}
					<select class="ez-control" aria-label="Target {i + 1}" bind:value={c.to}>
						{#each targets as t (t)}<option value={t}>{t}</option>{/each}
					</select>
					{#if c.when !== ELSE}
						<Button
							size="sm"
							variant="ghost"
							aria-label="Move condition {i + 1} up"
							disabled={i === 0}
							onclick={() => move(i, -1)}>↑</Button
						>
						<Button
							size="sm"
							variant="ghost"
							aria-label="Move condition {i + 1} down"
							disabled={i + 1 >= next.conds.length || next.conds[i + 1].when === ELSE}
							onclick={() => move(i, 1)}>↓</Button
						>
					{/if}
					<Button
						size="sm"
						variant="ghost"
						aria-label="Remove condition {i + 1}"
						onclick={() => remove(i)}>×</Button
					>
				</li>
				{#if problem}<li class="wf-edges__problem" role="alert">{problem}</li>{/if}
			{/each}
		</ol>
		<div class="wf-edges__actions">
			<Button size="sm" onclick={add}>Add condition</Button>
			{#if !next.conds.some((c) => c.when === ELSE)}
				<Button
					size="sm"
					onclick={() => {
						if (next?.kind === 'cond')
							next = { kind: 'cond', conds: [...next.conds, { when: ELSE, to: targets[0] ?? '' }] };
					}}>Add else</Button
				>
			{/if}
		</div>
	{/if}
</fieldset>

<style>
	.wf-edges {
		display: grid;
		gap: var(--ez-space-2);
		border: 1px solid var(--ez-border);
		border-radius: var(--ez-radius-sm);
		padding: var(--ez-space-3);
	}
	.wf-edges__list {
		display: grid;
		gap: var(--ez-space-2);
		margin: 0;
		padding-left: var(--ez-space-5);
	}
	.wf-edges__row {
		display: flex;
		gap: var(--ez-space-1);
		align-items: center;
	}
	.wf-edges__row > :global(.ez-control) {
		min-width: 0;
		flex: 1;
	}
	.wf-edges__else {
		flex: 1;
		font-family: monospace;
		color: var(--ez-text-muted);
	}
	.wf-edges__problem {
		list-style: none;
		color: var(--ez-danger-fg);
		font-size: 0.8rem;
	}
	.wf-edges__actions {
		display: flex;
		gap: var(--ez-space-2);
	}
</style>
