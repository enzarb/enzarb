export interface ComponentInfo {
	/** Key in ENZARB_COMPONENT_VERSIONS.components. */
	name: string;
	label: string;
	/** GitHub repository name under the enzarb organisation. */
	repo: string;
}

/** What the About page lists; a component the chart omits still shows, as "unknown". */
export const COMPONENTS: readonly ComponentInfo[] = [
	{ name: 'enzarb-ui', label: 'UI', repo: 'enzarb-ui' },
	{ name: 'enzarb-workflows', label: 'Workflows', repo: 'enzarb-workflows' },
	{ name: 'enzarb-connectors', label: 'Connectors', repo: 'enzarb-connectors' },
	{ name: 'enzarb-realtime', label: 'Realtime', repo: 'enzarb-realtime' },
	{ name: 'enzarb-sandbox-proxy', label: 'Sandbox proxy', repo: 'enzarb-sandbox-proxy' },
	{ name: 'enzarb-sandbox-operator', label: 'Operator', repo: 'enzarb-sandbox-operator' },
	{ name: 'enzarb-sandbox-container', label: 'Sandbox', repo: 'enzarb-sandbox-container' },
	{ name: 'enzarb-updater', label: 'Updater', repo: 'enzarb-updater' }
];

export const UNKNOWN_VERSION = 'unknown';

export const githubRepoUrl = (repo: string) => `https://github.com/enzarb/${repo}`;

/** Strips surrounding whitespace and a leading `v`, so `v1.2.3` and `1.2.3` compare equal. */
export function normalizeTag(tag: string): string {
	return tag.trim().replace(/^v(?=\d)/i, '');
}

/** The tag to show, or "unknown" when the chart did not provide one. */
export function displayVersion(tag: string | undefined | null): string {
	const trimmed = tag?.trim();
	return trimmed ? trimmed : UNKNOWN_VERSION;
}

/** Index of the release whose tag equals the deployed tag, or -1 (non-release tags match nothing). */
export function matchRelease(
	deployedTag: string | undefined | null,
	releases: readonly { tag: string }[]
): number {
	const wanted = normalizeTag(deployedTag ?? '');
	if (!wanted) return -1;
	return releases.findIndex((r) => normalizeTag(r.tag) === wanted);
}
