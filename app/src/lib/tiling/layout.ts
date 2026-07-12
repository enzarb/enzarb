export type TabKind = 'file' | 'terminal' | 'agent';

export interface Tab {
	kind: TabKind;
	id: string;
	label?: string;
	/** Project this tab belongs to. Every tab — file, terminal, agent — is
	 * scoped to a single project so one tiling workspace can hold panes from
	 * many projects side by side. Legacy layouts persisted before these fields
	 * existed are stamped on load (project mode) or during the global-layout
	 * migration. */
	namespace: string;
	project: string;
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
			const key = `${t.namespace}/${t.project}:${t.kind}:${t.id}`;
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

// ─── Global (cross-project) layout ──────────────────────────────────────────
//
// A single per-user tiling workspace whose tabs each carry their own
// namespace/project, so panes from many projects can live side by side. The
// per-project loadLayout/saveLayout above remain for the legacy single-project
// tiling route.

const GLOBAL_LAYOUT_KEY = 'enzarb:tiling:layout:global';
const LEGACY_LAYOUT_PREFIX = 'enzarb:tiling:layout:';

/** Ensures every tab in the tree carries a namespace/project, stamping the
 * given defaults onto any that predate those fields. Non-mutating. */
export function stampTabs(node: PaneNode, namespace: string, project: string): PaneNode {
	return mapPaneLeaves(node, (leaf) => {
		let changed = false;
		const tabs = leaf.tabs.map((t) => {
			if (t.namespace && t.project) return t;
			changed = true;
			return { ...t, namespace: t.namespace || namespace, project: t.project || project };
		});
		return changed ? { ...leaf, tabs } : leaf;
	});
}

/** One-time, non-destructive migration: fold any legacy per-project layouts
 * (`enzarb:tiling:layout:<ns>/<proj>`) into a single global layout, stamping
 * each tab with its owning project. File tabs land in the left region, terminal
 * and agent tabs in the right region. Legacy keys are left in place so the old
 * per-project route keeps working. */
function migrateLegacyLayouts(): TilingLayout {
	const merged: TilingLayout = structuredClone(DEFAULT_LAYOUT);
	const leftLeaf = merged.left.panes as LeafPane;
	const rightLeaf = merged.right.panes as LeafPane;
	const seen = new Set<string>();
	try {
		for (let i = 0; i < localStorage.length; i++) {
			const key = localStorage.key(i);
			if (!key || !key.startsWith(LEGACY_LAYOUT_PREFIX)) continue;
			if (key === GLOBAL_LAYOUT_KEY) continue;
			const suffix = key.slice(LEGACY_LAYOUT_PREFIX.length); // "<ns>/<proj>"
			const slash = suffix.indexOf('/');
			if (slash < 0) continue;
			const namespace = suffix.slice(0, slash);
			const project = suffix.slice(slash + 1);
			let parsed: TilingLayout;
			try {
				parsed = JSON.parse(localStorage.getItem(key) ?? '');
			} catch {
				continue;
			}
			const tabs = [
				...collectTabs(parsed.left?.panes ?? { type: 'leaf', tabs: [], activeTab: 0 }),
				...collectTabs(parsed.right?.panes ?? { type: 'leaf', tabs: [], activeTab: 0 })
			];
			for (const tab of tabs) {
				const stamped: Tab = { ...tab, namespace, project };
				const dedupeKey = `${namespace}/${project}:${tab.kind}:${tab.id}`;
				if (seen.has(dedupeKey)) continue;
				seen.add(dedupeKey);
				(tab.kind === 'file' ? leftLeaf : rightLeaf).tabs.push(stamped);
			}
		}
	} catch {}
	return merged;
}

export function loadGlobalLayout(): TilingLayout {
	try {
		const raw = localStorage.getItem(GLOBAL_LAYOUT_KEY);
		if (!raw) {
			// First global load — seed from any legacy per-project layouts.
			const migrated = migrateLegacyLayouts();
			saveGlobalLayout(migrated);
			return migrated;
		}
		const parsed = JSON.parse(raw);
		const layout: TilingLayout = { ...structuredClone(DEFAULT_LAYOUT), ...parsed };
		layout.left.panes = dedupeTabs(layout.left.panes);
		layout.right.panes = dedupeTabs(layout.right.panes);
		return layout;
	} catch {
		return structuredClone(DEFAULT_LAYOUT);
	}
}

export function saveGlobalLayout(layout: TilingLayout): void {
	try {
		localStorage.setItem(GLOBAL_LAYOUT_KEY, JSON.stringify(layout));
	} catch {}
}

// ─── Project colors ─────────────────────────────────────────────────────────

const PROJECT_COLORS_KEY = 'enzarb:project-colors';

/** Curated palette used to auto-assign a stable default color per project. */
export const PROJECT_COLOR_PALETTE = [
	'#6c6cff', // indigo
	'#3dba7a', // green
	'#f5a623', // amber
	'#e05252', // red
	'#58a6ff', // blue
	'#c471ed', // violet
	'#2dd4bf', // teal
	'#f472b6' // pink
];

function projectColorId(namespace: string, project: string): string {
	return `${namespace}/${project}`;
}

/** Deterministic hash → palette index, so a project keeps the same default
 * color across reloads without any stored state (no Math.random / Date.now). */
function autoProjectColor(namespace: string, project: string): string {
	const id = projectColorId(namespace, project);
	let hash = 0;
	for (let i = 0; i < id.length; i++) {
		hash = (hash * 31 + id.charCodeAt(i)) | 0;
	}
	const idx = Math.abs(hash) % PROJECT_COLOR_PALETTE.length;
	return PROJECT_COLOR_PALETTE[idx];
}

export function loadProjectColors(): Record<string, string> {
	try {
		const raw = localStorage.getItem(PROJECT_COLORS_KEY);
		if (!raw) return {};
		const parsed = JSON.parse(raw);
		return parsed && typeof parsed === 'object' ? parsed : {};
	} catch {
		return {};
	}
}

export function saveProjectColor(namespace: string, project: string, color: string): void {
	try {
		const colors = loadProjectColors();
		colors[projectColorId(namespace, project)] = color;
		localStorage.setItem(PROJECT_COLORS_KEY, JSON.stringify(colors));
	} catch {}
}

/** The user's chosen color for a project, or a deterministic palette default. */
export function getProjectColor(namespace: string, project: string): string {
	const colors = loadProjectColors();
	return colors[projectColorId(namespace, project)] ?? autoProjectColor(namespace, project);
}

function cwdKey(namespace: string, project: string): string {
	return `enzarb:tiling:cwd:${namespace}/${project}`;
}

/** Last working directory the user chose when creating a pane in this project. */
export function loadLastCwd(namespace: string, project: string): string {
	try {
		return localStorage.getItem(cwdKey(namespace, project)) ?? '';
	} catch {
		return '';
	}
}

export function saveLastCwd(namespace: string, project: string, cwd: string): void {
	try {
		if (cwd) localStorage.setItem(cwdKey(namespace, project), cwd);
		else localStorage.removeItem(cwdKey(namespace, project));
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

/** Removes any tab for which `keep` returns false, clamping activeTab. Used by
 * mount-time validation, where liveness must be judged per project (terminal
 * and session ids are only unique within a project). */
export function filterTabs(node: PaneNode, keep: (tab: Tab) => boolean): PaneNode {
	return mapPaneLeaves(node, (leaf) => {
		const tabs = leaf.tabs.filter(keep);
		if (tabs.length === leaf.tabs.length) return leaf;
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
