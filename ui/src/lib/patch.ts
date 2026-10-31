/**
 * Unified diff hunks as GitHub returns them in the `patch` field of `pulls/{n}/files`: the
 * `@@ -a,b +c,d @@` header lines and the lines under them, without the `---`/`+++` file header.
 */

export type DiffLineKind = 'add' | 'del' | 'ctx' | 'hunk' | 'note';

export interface DiffLine {
	kind: DiffLineKind;
	/** The line without its leading `+`, `-` or space (the whole header for `hunk`). */
	text: string;
	/** Line number in the old file; null for added lines and headers. */
	oldNo: number | null;
	/** Line number in the new file; null for removed lines and headers. */
	newNo: number | null;
}

/** Files with a patch longer than this are not shown inline. */
export const MAX_PATCH_CHARS = 60_000;

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

/** Parse a `patch` into display lines with old and new line numbers. */
export function parsePatch(patch: string): DiffLine[] {
	const out: DiffLine[] = [];
	let oldNo = 0;
	let newNo = 0;
	const lines = patch.split('\n');
	// A trailing newline leaves one empty element: it is not a context line.
	if (lines.at(-1) === '') lines.pop();
	for (const line of lines) {
		const h = HUNK.exec(line);
		if (h) {
			oldNo = Number(h[1]);
			newNo = Number(h[2]);
			out.push({ kind: 'hunk', text: line, oldNo: null, newNo: null });
			continue;
		}
		const c = line[0];
		if (c === '+') out.push({ kind: 'add', text: line.slice(1), oldNo: null, newNo: newNo++ });
		else if (c === '-') out.push({ kind: 'del', text: line.slice(1), oldNo: oldNo++, newNo: null });
		else if (c === '\\')
			out.push({ kind: 'note', text: line.slice(1).trim(), oldNo: null, newNo: null });
		else if (out.length > 0)
			out.push({ kind: 'ctx', text: line.slice(1), oldNo: oldNo++, newNo: newNo++ });
	}
	return out;
}

export interface PullFile {
	filename: string;
	status: string;
	additions: number;
	deletions: number;
	/** Absent for binary files and for diffs GitHub considers too large. */
	patch?: string;
}

/** Why a file cannot be shown inline, or null when it can. */
export function notInline(f: PullFile): string | null {
	if (f.patch === undefined || f.patch === '')
		return f.additions + f.deletions === 0 ? 'no content changes' : 'too large to show inline';
	return f.patch.length > MAX_PATCH_CHARS ? 'too large to show inline' : null;
}
