import { getContext, setContext } from 'svelte';
import type { ProjectView } from '#lib/project-types.ts';

/**
 * State the project layout shares with its tab pages. `project` is always loaded when a tab renders
 * (the layout owns the loading and error states); `trackingResources` keeps the layout's poll running
 * while a saved sandbox-resources change is still being applied.
 */
export type ProjectContext = {
	readonly project: ProjectView;
	trackingResources: boolean;
};

const KEY = Symbol('enzarb.project');

export const setProjectContext = (ctx: ProjectContext) => setContext(KEY, ctx);
export const getProjectContext = () => getContext<ProjectContext>(KEY);

/**
 * Tabs of the project page. Each is a child route of `/projects/[name]`; Overview is the index.
 * Chat exists only behind the `ENZARB_UI_CHAT` flag: use `projectTabs` to get the visible ones.
 */
export const PROJECT_TABS = [
	{ slug: '', label: 'Overview' },
	{ slug: 'terminal', label: 'Terminal' },
	{ slug: 'chat', label: 'Chat' },
	{ slug: 'settings', label: 'Settings' }
] as const;

/** The tabs to show; Chat only when the flag is on. */
export const projectTabs = (opts: { chat: boolean }) =>
	PROJECT_TABS.filter((t) => t.slug !== 'chat' || opts.chat);
