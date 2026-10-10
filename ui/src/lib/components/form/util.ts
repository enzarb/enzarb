/** A validation issue as the remote-function `fields.x.issues()` returns it, or a bare message. */
export type Issue = string | { message: string };
export type Issues = readonly Issue[] | undefined | null;

export interface Option {
	value: string;
	label: string;
	disabled?: boolean;
	/** Secondary text shown under the label (e.g. why an option is disabled). */
	description?: string;
	/** Combobox only: options sharing a group are listed under one heading. */
	group?: string;
}

export function messages(issues: Issues): string[] {
	return (issues ?? []).map((i) => (typeof i === 'string' ? i : i.message));
}

/** Ids of the help / error blocks that exist, for `aria-describedby`. */
export function describedBy(
	id: string,
	help: string | undefined,
	issues: Issues
): string | undefined {
	const ids = [help ? `${id}-help` : '', messages(issues).length ? `${id}-error` : ''].filter(
		Boolean
	);
	return ids.length ? ids.join(' ') : undefined;
}
