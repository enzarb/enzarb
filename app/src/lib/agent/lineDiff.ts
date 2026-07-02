export interface DiffLine {
	kind: 'context' | 'add' | 'remove';
	text: string;
}

/**
 * Minimal LCS-based line diff — good enough for the typical small edits an
 * agent tool call produces. Avoids pulling in a diff library; not intended
 * for huge files (O(n*m)).
 */
export function diffLines(oldText: string | null, newText: string): DiffLine[] {
	const a = (oldText ?? '').split('\n');
	const b = newText.split('\n');
	if (oldText === null) return b.map((text) => ({ kind: 'add', text }));

	const n = a.length;
	const m = b.length;
	const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
	for (let i = n - 1; i >= 0; i--) {
		for (let j = m - 1; j >= 0; j--) {
			lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
		}
	}

	const out: DiffLine[] = [];
	let i = 0;
	let j = 0;
	while (i < n && j < m) {
		if (a[i] === b[j]) {
			out.push({ kind: 'context', text: a[i] });
			i++;
			j++;
		} else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
			out.push({ kind: 'remove', text: a[i] });
			i++;
		} else {
			out.push({ kind: 'add', text: b[j] });
			j++;
		}
	}
	while (i < n) out.push({ kind: 'remove', text: a[i++] });
	while (j < m) out.push({ kind: 'add', text: b[j++] });
	return out;
}
