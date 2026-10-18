/** The updater's `/status`: the chart oldie runs versus the latest published. */
export interface ChartRollout {
	/** The chart version the helm-controller last installed; null when unreadable. */
	installed: string | null;
	/** `HelmChart.spec.version`, what the updater asked for. */
	desired: string | null;
	latest: string | null;
	/** The upgrade to `desired` is failing. */
	stuck: boolean;
	/** When recent chart versions finished installing; empty from an updater without history. */
	releases: ChartRelease[];
}

export interface AboutRelease {
	tag: string;
	name: string;
	publishedAt: string | null;
	url: string;
	/** Sanitized HTML (rendered server-side). */
	html: string;
}

export interface AboutComponent {
	name: string;
	label: string;
	/** Null for components found only in the chart config. */
	repoUrl: string | null;
	/** The deployed tag, or "unknown". */
	deployed: string;
	/** Null when the release notes could not be fetched (or there is no repo to fetch from). */
	releases: AboutRelease[] | null;
}

/** One entry of the updater's `releases`: when a chart version finished installing. */
export interface ChartRelease {
	version: string;
	/** RFC 3339. */
	deployedAt: string;
}

/**
 * Merge-to-deployed timing of one chart release. Every stage can be missing (null): a release
 * without a recorded deploy, or one that needed no sandbox roll, still shows the stages it has.
 */
export interface ReleaseTiming {
	version: string;
	url: string;
	/** Release time, standing in for the merge that triggered it. */
	publishedAt: string | null;
	deployedAt: string | null;
	sandboxAppliedAt: string | null;
	/** `deployedAt - publishedAt`; null when either is missing or the clocks disagree. */
	mergeToDeployedSeconds: number | null;
	/** Both timestamps exist but `deployedAt` is before `publishedAt`. */
	skew: boolean;
	/** `sandboxAppliedAt - deployedAt`. */
	deployToSandboxSeconds: number | null;
}
