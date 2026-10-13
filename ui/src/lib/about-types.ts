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
