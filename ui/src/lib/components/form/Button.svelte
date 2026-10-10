<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';

	let {
		variant = 'default',
		type = 'button',
		children,
		...rest
	}: {
		variant?: 'default' | 'primary' | 'danger';
		children: Snippet;
	} & HTMLButtonAttributes = $props();
</script>

<button {...rest} {type} class:primary={variant === 'primary'} class:danger={variant === 'danger'}>
	{@render children()}
</button>
