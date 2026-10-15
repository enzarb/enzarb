import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { NAMED_PROMPTS } from './definition.ts';
import { BUILTIN_PROMPTS } from './builtin-prompts.ts';

const engine = new URL('../../../../workflows/crates/engine/', import.meta.url);

describe('built-in prompt copy', () => {
	it('equals the engine templates (rerun node scripts/sync-builtin-prompts.mjs)', () => {
		expect(Object.keys(BUILTIN_PROMPTS).sort()).toEqual([...NAMED_PROMPTS].sort());
		for (const name of NAMED_PROMPTS)
			expect(BUILTIN_PROMPTS[name], name).toBe(
				readFileSync(new URL(`prompts/${name}.md`, engine), 'utf8')
			);
	});

	it('names every prompt the engine resolves by name', () => {
		const src = readFileSync(new URL('src/machine/prompt.rs', engine), 'utf8');
		const body = src.slice(src.indexOf('pub fn named_prompt'));
		const arms = body.slice(0, body.indexOf('_ => return None'));
		const names = [...arms.matchAll(/"([a-z_]+)" =>/g)].map((m) => m[1]);
		expect(names.sort()).toEqual([...NAMED_PROMPTS].sort());
	});
});
