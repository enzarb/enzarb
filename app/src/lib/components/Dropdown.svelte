<script lang="ts">
	import type { Snippet } from 'svelte';

	// A small popover menu: a trigger button and a right-aligned panel of items.
	// Closes on outside-click and Escape — the fixes the hand-rolled menus lack.
	let {
		label,
		align = 'right',
		triggerClass = 'btn btn-subtle',
		title = undefined,
		disabled = false,
		trigger,
		children
	}: {
		label?: string;
		align?: 'left' | 'right';
		triggerClass?: string;
		title?: string;
		disabled?: boolean;
		/** Optional custom trigger content; falls back to `label`. */
		trigger?: Snippet;
		/** Menu items. Rendered inside the panel; use <button class="dropdown-item">. */
		children: Snippet<[{ close: () => void }]>;
	} = $props();

	let open = $state(false);
	let root: HTMLDivElement | undefined = $state();

	function close() {
		open = false;
	}

	// Global listeners only while open, torn down when closed.
	$effect(() => {
		if (!open) return;
		const onPointer = (e: MouseEvent) => {
			if (root && !root.contains(e.target as Node)) close();
		};
		const onKey = (e: KeyboardEvent) => {
			if (e.key === 'Escape') close();
		};
		window.addEventListener('mousedown', onPointer);
		window.addEventListener('keydown', onKey);
		return () => {
			window.removeEventListener('mousedown', onPointer);
			window.removeEventListener('keydown', onKey);
		};
	});
</script>

<div class="dropdown" class:open bind:this={root}>
	<button
		class={triggerClass}
		type="button"
		{title}
		{disabled}
		aria-haspopup="menu"
		aria-expanded={open}
		onclick={() => (open = !open)}
	>
		{#if trigger}{@render trigger()}{:else}{label}{/if}
	</button>
	{#if open}
		<div class="dropdown-menu" class:left={align === 'left'} role="menu">
			{@render children({ close })}
		</div>
	{/if}
</div>

<style>
	.dropdown {
		position: relative;
		display: inline-flex;
	}
	.dropdown-menu {
		position: absolute;
		right: 0;
		top: calc(100% + 4px);
		min-width: 190px;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius);
		box-shadow: var(--shadow);
		z-index: 20;
		overflow: hidden;
		padding: 0.25rem;
	}
	.dropdown-menu.left {
		right: auto;
		left: 0;
	}
	.dropdown-menu :global(.dropdown-item) {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		width: 100%;
		padding: 0.45rem 0.6rem;
		background: none;
		border: none;
		border-radius: 4px;
		color: var(--color-text);
		font-size: 13px;
		cursor: pointer;
		text-align: left;
		white-space: nowrap;
	}
	.dropdown-menu :global(.dropdown-item:hover) {
		background: var(--color-surface-2);
	}
	.dropdown-menu :global(.dropdown-item:disabled) {
		opacity: 0.5;
		cursor: default;
	}
	.dropdown-menu :global(.dropdown-item.danger) {
		color: var(--color-danger);
	}
</style>
