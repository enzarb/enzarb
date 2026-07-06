<script lang="ts">
	import type { PaneNode, LeafPane, Tab } from './layout';
	import TilingPane from './TilingPane.svelte';
	import TilingSplitHandle from './TilingSplitHandle.svelte';
	import TilingRegion from './TilingRegion.svelte';

	interface Props {
		node: PaneNode;
		nodeId: string;
		regionKind: 'left' | 'right';
		agentBase: string;
		namespace: string;
		project: string;
		dragging: boolean;
		dragSource: { paneId: string; tabIndex: number } | null;
		onUpdate: (nodeId: string, updated: PaneNode) => void;
		onTabClose: (paneId: string, tabIndex: number) => void;
		onTabSelect: (paneId: string, tabIndex: number) => void;
		onSplit: (paneId: string, direction: 'h' | 'v', side: 'before' | 'after') => void;
		onAddTab: (paneId: string, tab: Tab) => void;
		onTabDragStart: (paneId: string, tabIndex: number) => void;
		onTabDrop: (targetPaneId: string, zone: 'top' | 'bottom' | 'left' | 'right' | 'center') => void;
		onRatioChange: (nodeId: string, ratio: number) => void;
	}

	let { node, nodeId, regionKind, agentBase, namespace, project, dragging, dragSource, onUpdate, onTabClose, onTabSelect, onSplit, onAddTab, onTabDragStart, onTabDrop, onRatioChange }: Props = $props();

	function handleRatioDrag(delta: number) {
		if (node.type !== 'split') return;
		const newRatio = Math.max(0.1, Math.min(0.9, node.ratio + delta));
		onRatioChange(nodeId, newRatio);
	}
</script>

{#if node.type === 'leaf'}
	<TilingPane
		pane={node}
		paneId={(node as any).__id ?? nodeId}
		{regionKind}
		{agentBase}
		{namespace}
		{project}
		{dragging}
		{onTabClose}
		{onTabSelect}
		{onSplit}
		{onAddTab}
		{onTabDragStart}
		{onTabDrop}
	/>
{:else}
	<div class="split-container {node.direction}" style={node.direction === 'h'
		? `--ratio: ${node.ratio}`
		: `--ratio: ${node.ratio}`}>
		<div class="split-child" style={node.direction === 'h'
			? `flex: 0 0 calc(var(--ratio) * 100%); min-width: 0;`
			: `flex: 0 0 calc(var(--ratio) * 100%); min-height: 0;`}>
			<TilingRegion
				node={node.children[0]}
				nodeId="{nodeId}-0"
				{regionKind}
				{agentBase}
				{namespace}
				{project}
				{dragging}
				{dragSource}
				{onUpdate}
				{onTabClose}
				{onTabSelect}
				{onSplit}
				{onAddTab}
				{onTabDragStart}
				{onTabDrop}
				{onRatioChange}
			/>
		</div>
		<TilingSplitHandle
			direction={node.direction}
			onDrag={(delta) => handleRatioDrag(delta)}
		/>
		<div class="split-child" style="flex: 1; min-width: 0; min-height: 0; overflow: hidden;">
			<TilingRegion
				node={node.children[1]}
				nodeId="{nodeId}-1"
				{regionKind}
				{agentBase}
				{namespace}
				{project}
				{dragging}
				{dragSource}
				{onUpdate}
				{onTabClose}
				{onTabSelect}
				{onSplit}
				{onAddTab}
				{onTabDragStart}
				{onTabDrop}
				{onRatioChange}
			/>
		</div>
	</div>
{/if}

<style>
	.split-container { display: flex; width: 100%; height: 100%; overflow: hidden; }
	.split-container.h { flex-direction: row; }
	.split-container.v { flex-direction: column; }
	.split-child { display: flex; overflow: hidden; }
</style>
