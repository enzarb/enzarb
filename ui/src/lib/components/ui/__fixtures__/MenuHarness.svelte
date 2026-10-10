<script lang="ts">
	import DropdownMenu from '../DropdownMenu.svelte';
	let picked = $state<string[]>([]);
	let value = $state('system');
</script>

<DropdownMenu
	label="Open menu"
	entries={[
		{ type: 'item', label: 'First', onselect: () => picked.push('first') },
		{ type: 'item', label: 'Second', onselect: () => picked.push('second') },
		{ type: 'separator' },
		{
			type: 'radio',
			label: 'Appearance',
			value,
			onchange: (v) => (value = v),
			options: [
				{ value: 'system', label: 'System' },
				{ value: 'dark', label: 'Dark' }
			]
		}
	]}
>
	{#snippet trigger()}Menu{/snippet}
</DropdownMenu>

<output>picked: {picked.join(',')}</output>
