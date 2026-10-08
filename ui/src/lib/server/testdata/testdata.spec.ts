import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const read = (f: string) => readFileSync(new URL(f, import.meta.url));

/** `<sha256>  <copy name>` lines written by scripts/sync-workflows-sql.sh. */
function recorded(): Record<string, string> {
	return Object.fromEntries(
		read('./workflows-sql.sha256')
			.toString('utf8')
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

	it('records a hash for every copied file', () => {
		expect(Object.keys(hashes).sort()).toEqual(['workflows_api.sql', 'workflows_schema.sql']);
	});

	for (const name of ['workflows_schema.sql', 'workflows_api.sql']) {
		it(`${name} matches the enzarb-workflows migration it copies`, () => {
			const actual = createHash('sha256')
				.update(read(`./${name}`))
				.digest('hex');
			expect(actual, `run scripts/sync-workflows-sql.sh to refresh ${name}`).toBe(hashes[name]);
		});
	}
});
