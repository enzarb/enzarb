<script lang="ts">
	import { type Snippet } from 'svelte';

	interface Props {
		children: Snippet;
		content: Snippet;
		placement?: 'top' | 'bottom' | 'left' | 'right';
	}

	let { children, content, placement = 'top' }: Props = $props();

	let triggerEl = $state<HTMLElement | null>(null);
	let tooltipEl = $state<HTMLElement | null>(null);
	let visible = $state(false);
	let style = $state('');

	function position() {
		if (!triggerEl || !tooltipEl) return;
		const tr = triggerEl.getBoundingClientRect();
		const tt = tooltipEl.getBoundingClientRect();
		const gap = 8;
		const vw = window.innerWidth;
		const vh = window.innerHeight;

		let top = 0, left = 0;

		if (placement === 'top') {
			top = tr.top - tt.height - gap;
			left = tr.left + tr.width / 2 - tt.width / 2;
		} else if (placement === 'bottom') {
			top = tr.bottom + gap;
			left = tr.left + tr.width / 2 - tt.width / 2;
		} else if (placement === 'left') {
			top = tr.top + tr.height / 2 - tt.height / 2;
			left = tr.left - tt.width - gap;
		} else {
			top = tr.top + tr.height / 2 - tt.height / 2;
			left = tr.right + gap;
		}

		// clamp to viewport
		left = Math.max(8, Math.min(left, vw - tt.width - 8));
		top = Math.max(8, Math.min(top, vh - tt.height - 8));

		style = `top:${top + window.scrollY}px;left:${left + window.scrollX}px`;
	}

	function show() {
		visible = true;
		// position after the tooltip renders
		requestAnimationFrame(position);
	}

	function hide() {
		visible = false;
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<span
	class="tooltip-trigger"
	bind:this={triggerEl}
	onmouseenter={show}
	onmouseleave={hide}
	onfocus={show}
	onblur={hide}
>
	{@render children()}
</span>

{#if visible}
	<div class="tooltip-bubble" bind:this={tooltipEl} {style}>
		{@render content()}
	</div>
{/if}

<style>
	.tooltip-trigger {
		display: contents;
	}
	.tooltip-bubble {
		position: absolute;
		z-index: 9999;
		background: var(--color-surface-2);
		border: 1px solid var(--color-border);
		border-radius: var(--radius);
		box-shadow: 0 4px 16px rgba(0, 0, 0, 0.5);
		padding: 0.5rem 0.75rem;
		font-size: 12px;
		color: var(--color-text);
		max-width: 400px;
		pointer-events: none;
		white-space: pre-wrap;
		word-break: break-word;
	}
</style>
