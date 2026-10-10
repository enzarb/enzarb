<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';
	import { buttonClass, type ButtonSize, type ButtonVariant } from '../ui/button.ts';

	/**
	 * Button. `variant`: primary | secondary | ghost | danger (`default` is the old name for
	 * secondary). `icon` makes it square (give it an `aria-label`). `loading` marks it busy:
	 * label kept, disabled, dimmed, aria-busy (no spinner); long operations get a step list instead. Anchors that look like buttons use LinkButton.
	 */
	let {
		variant = 'secondary',
		size = 'md',
		icon = false,
		loading = false,
		type = 'button',
		disabled,
		children,
		...rest
	}: {
		variant?: ButtonVariant | 'default';
		size?: ButtonSize;
		icon?: boolean;
		loading?: boolean;
		children: Snippet;
	} & HTMLButtonAttributes = $props();
</script>

<button
	{...rest}
	{type}
	class={buttonClass(variant === 'default' ? 'secondary' : variant, size, icon, loading)}
	disabled={disabled || loading}
	aria-busy={loading || undefined}
>
	{@render children()}
</button>
