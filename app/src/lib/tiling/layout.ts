export type TabKind = 'file' | 'terminal' | 'agent';

export interface Tab {
	kind: TabKind;
	id: string;
	label?: string;
}

export interface LeafPane {
	type: 'leaf';
	tabs: Tab[];
	activeTab: number;
}

export interface SplitPane {
	type: 'split';
	direction: 'h' | 'v';
	ratio: number;
	children: [PaneNode, PaneNode];
}

export type PaneNode = LeafPane | SplitPane;

export interface SidebarState {
	/** Extra fraction of the main area given to the file viewer once a file is open. */
	viewerFraction: number;
	collapsed: boolean;
}

export interface RegionState {
	sidebar?: SidebarState;
	panes: PaneNode;
}

export interface TilingLayout {
	divider: number;
	left: RegionState;
	right: RegionState;
}

const DEFAULT_LAYOUT: TilingLayout = {
	divider: 0.25,
	left: {
		sidebar: { viewerFraction: 0.3, collapsed: false },
		panes: { type: 'leaf', tabs: [], activeTab: 0 }
	},
	right: {
		panes: { type: 'leaf', tabs: [], activeTab: 0 }
	}
};

function layoutKey(namespace: string, project: string): string {
	return `enzarb:tiling:layout:${namespace}/${project}`;
}

/** Drops duplicate (kind, id) tabs within each leaf, keeping the first and
 * clamping activeTab — guards against layouts persisted before tab-add
 * gained its own dedup, which would otherwise crash the keyed tab {#each}. */
function dedupeTabs(node: PaneNode): PaneNode {
	return mapPaneLeaves(node, (leaf) => {
		const seen = new Set<string>();
		const tabs = leaf.tabs.filter((t) => {
			const key = `${t.kind}:${t.id}`;
			if (seen.has(key)) return false;
			seen.add(key);
			return true;
		});
		if (tabs.length === leaf.tabs.length) return leaf;
		return { ...leaf, tabs, activeTab: Math.min(leaf.activeTab, Math.max(0, tabs.length - 1)) };
	});
}

export function loadLayout(namespace: string, project: string): TilingLayout {
	try {
		const raw = localStorage.getItem(layoutKey(namespace, project));
		if (!raw) return structuredClone(DEFAULT_LAYOUT);
		const parsed = JSON.parse(raw);
		const layout: TilingLayout = { ...structuredClone(DEFAULT_LAYOUT), ...parsed };
		layout.left.panes = dedupeTabs(layout.left.panes);
		layout.right.panes = dedupeTabs(layout.right.panes);
		return layout;
	} catch {
		return structuredClone(DEFAULT_LAYOUT);
	}
}

export function saveLayout(namespace: string, project: string, layout: TilingLayout): void {
	try {
		localStorage.setItem(layoutKey(namespace, project), JSON.stringify(layout));
	} catch {}
}

export function collectTabs(node: PaneNode): Tab[] {
	if (node.type === 'leaf') return node.tabs;
	return [...collectTabs(node.children[0]), ...collectTabs(node.children[1])];
}

export function mapPaneLeaves(node: PaneNode, fn: (leaf: LeafPane) => LeafPane): PaneNode {
	if (node.type === 'leaf') return fn(node);
	return {
		...node,
		children: [mapPaneLeaves(node.children[0], fn), mapPaneLeaves(node.children[1], fn)]
	};
}

export function removeTabs(node: PaneNode, ids: Set<string>): PaneNode {
	return mapPaneLeaves(node, (leaf) => {
		const tabs = leaf.tabs.filter((t) => !ids.has(t.id));
		const activeTab = Math.min(leaf.activeTab, Math.max(0, tabs.length - 1));
		return { ...leaf, tabs, activeTab };
	});
}

export function addTabToLeaf(node: PaneNode, leafId: string, tab: Tab): PaneNode {
	return mapPaneLeaves(node, (leaf) => {
		if ((leaf as any).__id !== leafId) return leaf;
		return { ...leaf, tabs: [...leaf.tabs, tab], activeTab: leaf.tabs.length };
	});
}

export function splitLeaf(
	node: PaneNode,
	leafId: string,
	direction: 'h' | 'v',
	side: 'before' | 'after'
): PaneNode {
	if (node.type === 'leaf') {
		if ((node as any).__id !== leafId) return node;
		const newLeaf: LeafPane = { type: 'leaf', tabs: [], activeTab: 0 };
		return {
			type: 'split',
			direction,
			ratio: 0.5,
			children: side === 'after' ? [node, newLeaf] : [newLeaf, node]
		};
	}
	return {
		...node,
		children: [
			splitLeaf(node.children[0], leafId, direction, side),
			splitLeaf(node.children[1], leafId, direction, side)
		]
	};
}

export function countLeaves(node: PaneNode): number {
	if (node.type === 'leaf') return 1;
	return countLeaves(node.children[0]) + countLeaves(node.children[1]);
}

export function removeEmptyLeaves(node: PaneNode): PaneNode | null {
	if (node.type === 'leaf') {
		return node.tabs.length === 0 ? null : node;
	}
	const left = removeEmptyLeaves(node.children[0]);
	const right = removeEmptyLeaves(node.children[1]);
	if (!left && !right) return null;
	if (!left) return right;
	if (!right) return left;
	return { ...node, children: [left, right] };
}
