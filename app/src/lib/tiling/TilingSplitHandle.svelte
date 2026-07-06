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
</script>

<div
	bind:this={el}
	class="split-handle {direction}"
	class:dragging
	role="separator"
	aria-orientation={direction === 'h' ? 'vertical' : 'horizontal'}
	onmousedown={onMouseDown}
></div>

<style>
	.split-handle { flex-shrink: 0; background: var(--color-border); z-index: 10; user-select: none; }
	.split-handle.h { width: 4px; cursor: col-resize; }
	.split-handle.h:hover, .split-handle.h.dragging { background: var(--color-accent); }
	.split-handle.v { height: 4px; cursor: row-resize; }
	.split-handle.v:hover, .split-handle.v.dragging { background: var(--color-accent); }
</style>
