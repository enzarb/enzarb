import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { NAMED_PROMPTS } from './definition.ts';
import { BUILTIN_PROMPTS } from './builtin-prompts.ts';

const engine = new URL('../../../../workflows/crates/engine/', import.meta.url);

describe('built-in prompts', () => {
	it('loads every named template from the engine', () => {
		expect(NAMED_PROMPTS).toContain('plan_epic');
		for (const name of NAMED_PROMPTS) {
			expect(BUILTIN_PROMPTS[name], name).toBe(
				readFileSync(new URL(`prompts/${name}.md`, engine), 'utf8')
			);
			expect(BUILTIN_PROMPTS[name].trim().length, name).toBeGreaterThan(0);
		}
	});

	it('names every prompt the engine resolves by name', () => {
		const src = readFileSync(new URL('src/machine/prompt.rs', engine), 'utf8');
		const body = src.slice(src.indexOf('pub fn named_prompt'));
		const arms = body.slice(0, body.indexOf('_ => return None'));
		const names = [...arms.matchAll(/"([a-z_]+)" =>/g)].map((m) => m[1]);
		// `render_graph_prompt` resolves `plan_epic` itself (the plan template, epic flavour).
		expect(src).toContain('if prompt.trim() == "plan_epic"');
		names.push('plan_epic');
		expect(names.sort()).toEqual([...NAMED_PROMPTS].sort());
	});
});
