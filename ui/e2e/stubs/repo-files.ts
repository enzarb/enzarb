// Stand-in for src/lib/server/repo-files.ts in the e2e web server: the repository's default branch
// is a directory on disk (REPO_DIR), so a spec can seed, inspect and reset it from the test process.
// Blob shas are content hashes and a commit names the sha it replaces, as on GitHub.
import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import type { CommitResult, ReadResult } from '../../src/lib/server/repo-files.ts';

export { coAuthorTrailer, MAX_FILE_BYTES } from '../../src/lib/server/repo-files.ts';
export type { CommitResult, ReadResult, RepoFile } from '../../src/lib/server/repo-files.ts';

export const REPO_DIR = path.resolve(import.meta.dirname, '../../test-results/.repo');

const sha = (text: string) => createHash('sha1').update(text).digest('hex');
const file = (repo: string, p: string) => path.join(REPO_DIR, repo, p);

export async function readRepoFile(opts: { repo: string; path: string }): Promise<ReadResult> {
	try {
		const text = await readFile(file(opts.repo, opts.path), 'utf8');
		return { ok: true, file: { text, sha: sha(text) } };
	} catch {
		return { ok: true, file: null };
	}
}

export async function commitRepoFile(opts: {
	repo: string;
	path: string;
	text: string;
	sha: string | null;
}): Promise<CommitResult> {
	const cur = await readRepoFile(opts);
	const curSha = cur.ok ? (cur.file?.sha ?? null) : null;
	if (curSha !== opts.sha)
		return { ok: false, reason: 'conflict', status: 409, message: 'the file changed' };
	const target = file(opts.repo, opts.path);
	await mkdir(path.dirname(target), { recursive: true });
	await writeFile(target, opts.text);
	return {
		ok: true,
		sha: sha(opts.text),
		commitSha: sha(`commit:${opts.text}`),
		commitUrl: `https://github.com/${opts.repo}/commit/${sha(opts.text)}`
	};
}
