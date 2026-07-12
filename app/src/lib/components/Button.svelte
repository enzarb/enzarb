<script lang="ts">
	import type { Snippet } from 'svelte';

	type Variant = 'default' | 'primary' | 'danger' | 'subtle' | 'ghost' | 'danger-outline';
	type Size = 'xs' | 'sm' | 'md';

	let {
		variant = 'default',
		size = 'md',
		icon = false,
		href = undefined,
		type = 'button',
		disabled = false,
		title = undefined,
		class: extra = '',
		onclick = undefined,
		children
	}: {
		variant?: Variant;
		size?: Size;
		icon?: boolean;
		href?: string;
		type?: 'button' | 'submit' | 'reset';
		disabled?: boolean;
		title?: string;
		class?: string;
		onclick?: (e: MouseEvent) => void;
		children?: Snippet;
	} = $props();

	// Canonical class list, all defined once in app.css.
	const cls = $derived(
		[
			'btn',
			variant !== 'default' ? `btn-${variant}` : '',
			size !== 'md' ? `btn-${size}` : '',
			icon ? 'btn-icon' : '',
			extra
		]
			.filter(Boolean)
			.join(' ')
	);
</script>

{#if href}
	<a class={cls} {href} {title} aria-disabled={disabled} {onclick}>
		{@render children?.()}
	</a>
{:else}
	<button class={cls} {type} {disabled} {title} {onclick}>
		{@render children?.()}
	</button>
{/if}
