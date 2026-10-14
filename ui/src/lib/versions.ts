export interface ComponentInfo {
	/** Key in ENZARB_COMPONENT_VERSIONS.components (the chart sends the image/component names). */
	name: string;
	label: string;
	/** Directory in the enzarb monorepo; also the release tag prefix (`<dir>/vX.Y.Z`). */
	dir: string;
}

/** What the About page lists; a component the chart omits still shows, as "unknown". */
export const COMPONENTS: readonly ComponentInfo[] = [
	{ name: 'enzarb-ui', label: 'UI', dir: 'ui' },
	{ name: 'enzarb-workflows', label: 'Workflows', dir: 'workflows' },
	{ name: 'enzarb-connectors', label: 'Connectors', dir: 'connectors' },
	{ name: 'enzarb-realtime', label: 'Realtime', dir: 'realtime' },
	{ name: 'enzarb-sandbox-proxy', label: 'Sandbox proxy', dir: 'sandbox-proxy' },
	{ name: 'enzarb-sandbox-operator', label: 'Operator', dir: 'operator' },
	{ name: 'enzarb-sandbox-container', label: 'Sandbox', dir: 'sandbox-container' },
	{ name: 'enzarb-updater', label: 'Updater', dir: 'updater' }
];

export const UNKNOWN_VERSION = 'unknown';

/** The component's directory in the enzarb monorepo on GitHub. */
export const componentDirUrl = (dir: string) => `https://github.com/enzarb/enzarb/tree/main/${dir}`;

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

/** True only for a plain `https://github.com/...` URL (no credentials or port): safe to link. */
export function isGithubUrl(raw: string | null | undefined): raw is string {
	if (!raw) return false;
	try {
		const u = new URL(raw);
		return (
			u.protocol === 'https:' &&
			u.hostname === 'github.com' &&
			!u.username &&
			!u.password &&
			!u.port
		);
	} catch {
		return false;
	}
}
