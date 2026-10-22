<script lang="ts">
	import Checkbox from '../form/Checkbox.svelte';
	import FieldSet from '../form/FieldSet.svelte';
	import type { ModesCatalog } from '#lib/modes-types.ts';

	/** `tools`: null means every built-in tool; otherwise the allowed subset. */
	let {
		tools = $bindable(null),
		catalog,
		issues = []
	}: { tools?: string[] | null; catalog: ModesCatalog; issues?: string[] } = $props();

	const builtin = $derived(catalog.builtinTools.map((t) => t.name));
	// Unknown names stay visible (validation reports them) so they can be unchecked.
	const names = $derived([...builtin, ...(tools ?? []).filter((t) => !builtin.includes(t))]);
	const readOnly = (name: string) => catalog.builtinTools.find((t) => t.name === name)?.readOnly;

	function toggle(name: string, on: boolean) {
		const current = tools ?? [];
		tools = names.filter((n) => (n === name ? on : current.includes(n)));
	}
</script>

<FieldSet legend="Built-in tools">
	<Checkbox
		label="All built-in tools"
		help="New built-in tools are included automatically."
		bind:checked={
			() => tools === null, (on) => (tools = on ? null : builtin.filter((n) => readOnly(n)))
		}
	/>
	{#if tools !== null}
		{@const chosen = tools}
		<div class="ez-modes-tools">
			{#each names as name (name)}
				<Checkbox
					label={readOnly(name) ? `${name} (read-only)` : name}
					bind:checked={() => chosen.includes(name), (on) => toggle(name, on)}
				/>
			{/each}
		</div>
	{/if}
	{#each issues as msg, n (n)}<p class="ez-field__error">{msg}</p>{/each}
</FieldSet>
