// Fails if the old "work item" name reappears. Run: node scripts/check-naming.mjs (part of `npm run lint`).
// SQL names in workflows_api / app_api are a shared contract and stay; each use is allowlisted below.
import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const PATTERN = /work[ _-]?item/i;
const SKIP_DIRS = new Set([
	'node_modules',
	'.svelte-kit',
	'build',
	'.git',
	'migrations',
	'test-results'
]);
const SKIP_FILES = new Set(['package-lock.json']);

/** Lines of `file` matching `pattern` are allowed. `why` documents the contract being kept. */
export const ALLOWLIST = [
	{
		file: 'src/lib/live.ts',
		pattern: /work_item/,
		why: 'legacy realtime frame topic (wire contract)'
	},
	{
		file: 'src/lib/live.spec.ts',
		pattern: /work_item/,
		why: 'tests the legacy realtime frame topic'
	},
	{ file: 'src/lib/usage.ts', pattern: /work_item_v\.usage_totals/, why: 'SQL view name' },
	{ file: 'src/lib/server/push-store.ts', pattern: /work_item_id/, why: 'SQL parameter mapping' },
	{ file: 'README.md', pattern: /work_item_changed/, why: 'SQL NOTIFY channel name' },
	{ file: 'scripts/check-naming.mjs', pattern: /./, why: 'holds the pattern text' },
	{ file: 'src/lib/check-naming.spec.ts', pattern: /./, why: 'fixtures for the guard' }
];

/** @param {string} root @param {string} dir @returns {string[]} */
function walk(root, dir = root) {
	const out = [];
	for (const e of readdirSync(dir, { withFileTypes: true })) {
		if (e.isDirectory()) {
			if (!SKIP_DIRS.has(e.name)) out.push(...walk(root, join(dir, e.name)));
		} else out.push(relative(root, join(dir, e.name)));
	}
	return out;
}

/** @param {string} root @returns {string[]} */
function listFiles(root) {
	try {
		const raw = execFileSync(
			'git',
			['ls-files', '-z', '--cached', '--others', '--exclude-standard', '--', '.'],
			{
				cwd: root,
				encoding: 'utf8',
				stdio: ['ignore', 'pipe', 'ignore']
			}
		);
		return raw.split('\0').filter(Boolean);
	} catch {
		return walk(root);
	}
}

/**
 * Returns problem strings (`file:line: text`) for `root`; empty when clean.
 * @param {string} root
 * @param {{ file: string, pattern: RegExp, why: string }[]} allowlist
 * @returns {string[]}
 */
export function checkNaming(root, allowlist = ALLOWLIST) {
	const problems = [];
	const used = new Set();
	for (const file of listFiles(root)) {
		const parts = file.split('/');
		if (
			parts.some((/** @type {string} */ p) => SKIP_DIRS.has(p)) ||
			SKIP_FILES.has(parts.at(-1) ?? '')
		)
			continue;
		let text;
		try {
			text = readFileSync(join(root, file), 'utf8');
		} catch {
			continue;
		}
		if (text.includes('\0')) continue;
		text.split('\n').forEach((line, i) => {
			if (!PATTERN.test(line)) return;
			const hit = allowlist.find((a) => a.file === file && a.pattern.test(line));
			if (hit) used.add(hit);
			else problems.push(`${file}:${i + 1}: ${line.trim()}`);
		});
	}
	for (const a of allowlist) {
		if (!used.has(a)) problems.push(`stale allowlist entry: ${a.file} (${a.why})`);
	}
	return problems;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
	const root = fileURLToPath(new URL('..', import.meta.url));
	const problems = checkNaming(root);
	if (problems.length) {
		console.error(
			'Use "task", not "work item" (SQL names need an allowlist entry in scripts/check-naming.mjs):'
		);
		for (const p of problems) console.error(`  ${p}`);
		process.exit(1);
	}
}
