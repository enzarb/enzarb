<script lang="ts">
	import Button from '../form/Button.svelte';
	import FieldSet from '../form/FieldSet.svelte';
	import Select from '../form/Select.svelte';
	import TextField from '../form/TextField.svelte';

	/** Tool name or glob -> `allow` / `ask` / `deny`, edited as rows (order is not stored). */
	let {
		permissions = $bindable({}),
		values,
		issues = []
	}: { permissions?: Record<string, string>; values: string[]; issues?: string[] } = $props();

	// Rows keep half-typed keys (and duplicates) until they are valid; the record is derived from them.
	let rows = $state(Object.entries(permissions).map(([tool, value]) => ({ tool, value })));
	const options = $derived(values.map((v) => ({ value: v, label: v })));

	function sync() {
		const out: Record<string, string> = {};
		for (const r of rows) if (r.tool.trim()) out[r.tool.trim()] = r.value;
		permissions = out;
	}
	function add() {
		rows.push({ tool: '', value: values.includes('ask') ? 'ask' : (values[0] ?? '') });
	}
	function remove(i: number) {
		rows.splice(i, 1);
		sync();
	}
</script>

<FieldSet legend="Permissions">
	{#each rows as row, i (i)}
		<div class="ez-cluster ez-cluster--end">
			<TextField
				label="Tool or glob"
				placeholder="Bash, mcp__github__*"
				bind:value={row.tool}
				oninput={sync}
			/>
			<Select label="Decision" {options} bind:value={row.value} onchange={sync} />
			<Button variant="ghost" onclick={() => remove(i)}>Remove</Button>
		</div>
	{/each}
	{#if !rows.length}<p class="ez-muted">
			No overrides: ez asks before tools that change things.
		</p>{/if}
	<div><Button onclick={add}>Add permission</Button></div>
	{#each issues as msg, n (n)}<p class="ez-field__error">{msg}</p>{/each}
</FieldSet>
