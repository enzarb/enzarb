<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLAnchorAttributes } from 'svelte/elements';
	import { buttonClass, type ButtonSize, type ButtonVariant } from './button.ts';

	/** An <a> that looks like a Button (same variants and sizes); use for navigation, not actions. */
	let {
		variant = 'secondary',
		size = 'md',
		href,
		children,
		...rest
	}: {
		variant?: ButtonVariant;
		size?: ButtonSize;
		href: string;
		children: Snippet;
	} & HTMLAnchorAttributes = $props();
</script>

<a {...rest} {href} class={buttonClass(variant, size, false, false)}>
	{@render children()}
</a>
