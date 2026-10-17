import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { checkNaming } from '../../scripts/check-naming.mjs';

let root: string;
const put = (file: string, text: string) => {
	mkdirSync(join(root, file, '..'), { recursive: true });
	writeFileSync(join(root, file), text);
};

beforeEach(() => {
	root = mkdtempSync(join(tmpdir(), 'naming-'));
});
afterEach(() => rmSync(root, { recursive: true, force: true }));

describe('checkNaming', () => {
	it('passes a clean tree', () => {
		put('src/a.ts', 'const task = 1;\n');
		expect(checkNaming(root, [])).toEqual([]);
	});

	it.each(['WorkItem', 'work-item', 'a work item', 'workItemId', 'work_item'])(
		'fails on %s',
		(name) => {
			put('src/a.ts', `ok\n// ${name}\n`);
			expect(checkNaming(root, [])).toEqual([`src/a.ts:2: // ${name}`]);
		}
	);

	it('allows allowlisted lines only', () => {
		put('src/a.ts', 'work_item_v\nWorkItem\n');
		const allow = [{ file: 'src/a.ts', pattern: /work_item_v/, why: 'sql' }];
		expect(checkNaming(root, allow)).toEqual(['src/a.ts:2: WorkItem']);
	});

	it('fails on a stale allowlist entry', () => {
		put('src/a.ts', 'clean\n');
		const allow = [{ file: 'src/a.ts', pattern: /work_item/, why: 'sql' }];
		expect(checkNaming(root, allow)).toEqual(['stale allowlist entry: src/a.ts (sql)']);
	});

	it('ignores migrations', () => {
		put('migrations/0006.sql', 'work_item_id\n');
		expect(checkNaming(root, [])).toEqual([]);
	});
});
