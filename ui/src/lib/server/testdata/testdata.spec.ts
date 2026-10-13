import { createHash } from 'node:crypto';
import { readdirSync, readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const dir = new URL('./workflows/', import.meta.url);

/** `<sha256>  <migration name>` lines written by scripts/sync-workflows-sql.sh. */
function recorded(): Record<string, string> {
	return Object.fromEntries(
		readFileSync(new URL('./workflows-sql.sha256', import.meta.url), 'utf8')
			.split('\n')
			.filter(Boolean)
			.map((l) => {
				const [hash, name] = l.split(/\s+/);
				return [name, hash];
			})
	);
}

describe('workflows SQL copies', () => {
	const hashes = recorded();
	const names = readdirSync(dir)
		.filter((f) => f.endsWith('.sql'))
		.sort();

	it('records a hash for every copied migration and no others', () => {
		expect(names.length).toBeGreaterThan(0);
		expect(Object.keys(hashes).sort()).toEqual(names);
	});

	for (const name of names) {
		it(`${name} matches the enzarb-workflows migration it copies`, () => {
			const actual = createHash('sha256')
				.update(readFileSync(new URL(name, dir)))
				.digest('hex');
			expect(actual, `run scripts/sync-workflows-sql.sh to refresh ${name}`).toBe(hashes[name]);
		});
	}

	it('includes the migration that adds the merge action', () => {
		expect(names).toContain('20261011000003_submit_action_merge.sql');
	});
});
