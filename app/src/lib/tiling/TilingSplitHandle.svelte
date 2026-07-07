<script lang="ts">
	interface Props {
		direction: 'h' | 'v';
		onDrag: (delta: number, total: number) => void;
	}

	let { direction, onDrag }: Props = $props();

	let dragging = $state(false);
	let el: HTMLDivElement | undefined = $state();

	function onMouseDown(e: MouseEvent) {
		e.preventDefault();
		dragging = true;
		let lastPos = direction === 'h' ? e.clientX : e.clientY;
		const parentSize = direction === 'h'
			? (el?.parentElement?.clientWidth ?? window.innerWidth)
			: (el?.parentElement?.clientHeight ?? window.innerHeight);

		function onMouseMove(e: MouseEvent) {
			const pos = direction === 'h' ? e.clientX : e.clientY;
			const delta = (pos - lastPos) / parentSize;
			lastPos = pos;
			onDrag(delta, parentSize);
		}

		function onMouseUp() {
			dragging = false;
			window.removeEventListener('mousemove', onMouseMove);
			window.removeEventListener('mouseup', onMouseUp);
		}

		window.addEventListener('mousemove', onMouseMove);
		window.addEventListener('mouseup', onMouseUp);
	}

	// Keyboard equivalent of dragging: arrow keys matching the resize axis
	// nudge the split by a small step (larger with Shift), same delta/total
	// contract as the mouse path.
	function onKeyDown(e: KeyboardEvent) {
		const decreaseKey = direction === 'h' ? 'ArrowLeft' : 'ArrowUp';
		const increaseKey = direction === 'h' ? 'ArrowRight' : 'ArrowDown';
		if (e.key !== decreaseKey && e.key !== increaseKey) return;
		e.preventDefault();
		const parentSize = direction === 'h'
			? (el?.parentElement?.clientWidth ?? window.innerWidth)
			: (el?.parentElement?.clientHeight ?? window.innerHeight);
		const step = e.shiftKey ? 0.1 : 0.02;
		onDrag(e.key === increaseKey ? step : -step, parentSize);
	}
</script>

<!--
	svelte's a11y linter doesn't recognize role="separator" as an interactive
	role, but the WAI-ARIA Authoring Practices Guide explicitly calls for a
	resizable separator to be focusable (tabindex="0") and operable via arrow
	keys — exactly what this implements. Suppressing both rules here is a
	correct accessible pattern, not a workaround.
-->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
	bind:this={el}
	class="split-handle {direction}"
	class:dragging
	role="separator"
	tabindex="0"
	aria-orientation={direction === 'h' ? 'vertical' : 'horizontal'}
	onmousedown={onMouseDown}
	onkeydown={onKeyDown}
></div>

<style>
	.split-handle { flex-shrink: 0; background: var(--color-border); z-index: 10; user-select: none; }
	.split-handle.h { width: 4px; cursor: col-resize; }
	.split-handle.h:hover, .split-handle.h.dragging { background: var(--color-accent); }
	.split-handle.v { height: 4px; cursor: row-resize; }
	.split-handle.v:hover, .split-handle.v.dragging { background: var(--color-accent); }
</style>
