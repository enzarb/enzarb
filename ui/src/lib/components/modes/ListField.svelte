<script lang="ts">
	import TextArea from '../form/TextArea.svelte';

	/** A list of strings edited as one entry per line; blank lines are dropped. */
	let {
		label,
		help,
		issues,
		value = $bindable([]),
		disabled = false
	}: {
		label: string;
		help?: string;
		issues?: string[];
		value?: string[];
		disabled?: boolean;
	} = $props();

	// Local text so typing a blank line does not snap back; re-seeded when the list changes outside.
	let text = $state(value.join('\n'));
	let last = value.join('\n');
	$effect.pre(() => {
		const joined = value.join('\n');
		if (joined !== last) text = last = joined;
	});

	function oninput() {
		value = text
			.split('\n')
			.map((s) => s.trim())
			.filter(Boolean);
		last = value.join('\n');
	}
</script>

<TextArea
	{label}
	{help}
	{issues}
	{disabled}
	rows={Math.min(8, Math.max(2, value.length + 1))}
	bind:value={text}
	{oninput}
/>
