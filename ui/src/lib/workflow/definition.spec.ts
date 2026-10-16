import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
	applyToDocument,
	check,
	parseWorkflow,
	serialize,
	validate,
	type Definition
} from './definition.ts';
import { conditionContext, evalExpr, parseExpr, type Json } from './expr.ts';

const root = new URL('../../../../', import.meta.url);
const fixtures = JSON.parse(
	readFileSync(new URL('workflows/crates/graph/tests/fixtures/validate.json', root), 'utf8')
) as {
	cases: { name: string; yaml: string; parse_error: boolean; errors: string[] }[];
	item_conditions: [Json, Json, string, boolean][];
	expressions: { ctx: Json; eval: [string, boolean][]; reject: string[] };
};
const EXAMPLE = readFileSync(new URL('docs/workflows/examples/issue.yaml', root), 'utf8');

describe('shared fixtures (workflows/crates/graph/tests/fixtures/validate.json)', () => {
	for (const c of fixtures.cases) {
		it(c.name, () => {
			const r = check(c.yaml);
			if (c.parse_error) {
				expect(r.parse, 'expected a parse error').toBeTypeOf('string');
				return;
			}
			expect(r.parse).toBeUndefined();
			const msgs = r.problems.map((p) => p.message);
			expect(msgs).toHaveLength(c.errors.length);
			for (const w of c.errors)
				expect(
					msgs.some((m) => m.includes(w)),
					`${w} in ${msgs}`
				).toBe(true);
		});
	}

	it('evaluates conditions', () => {
		for (const [src, want] of fixtures.expressions.eval)
			expect(evalExpr(parseExpr(src), fixtures.expressions.ctx), src).toBe(want);
	});

	it('falls back to _item facts', () => {
		for (const [output, item, cond, want] of fixtures.item_conditions)
			expect(evalExpr(parseExpr(cond), conditionContext(output, item ?? undefined)), cond).toBe(
				want
			);
	});

	it('rejects conditions', () => {
		for (const src of fixtures.expressions.reject) expect(() => parseExpr(src), src).toThrow();
	});
});

describe('parseWorkflow', () => {
	it('keeps condition order and source text of keys', () => {
		const { def } = parseWorkflow(
			'version: 1\nname: t\non: [x]\nstart: a\nnodes:\n  a: {type: command, run: x, optional: true, next: {z: e, true: e, 2: e, b: e, else: e}}\n  e: {type: end, status: done}\n'
		);
		const a = def.nodes[0];
		expect(a.next).toEqual({
			kind: 'cond',
			conds: ['z', 'true', '2', 'b', 'else'].map((when) => ({ when, to: 'e' }))
		});
	});

	it('reads a prompt file reference', () => {
		const { def } = parseWorkflow(
			'version: 1\nname: t\non: [x]\nstart: a\nnodes:\n  a: {type: agent, prompt: {file: .enzarb/prompts/fix.md}, next: e}\n  e: {type: end, status: done}\n'
		);
		expect(def.nodes[0].prompt).toEqual({ file: '.enzarb/prompts/fix.md' });
		expect(validate(def)).toEqual([]);
	});

	it('rejects a prompt file outside .enzarb/prompts', () => {
		const r = check(
			'version: 1\nname: t\non: [x]\nstart: a\nnodes:\n  a: {type: agent, prompt: {file: ../secret.md}, next: e}\n  e: {type: end, status: done}\n'
		);
		expect(r.problems.map((p) => p.message)).toEqual([
			'node `a`: prompt file `../secret.md` must match .enzarb/prompts/<[A-Za-z0-9_-]+>.md'
		]);
	});

	it('ties problems to nodes', () => {
		const r = check(
			'version: 1\nname: t\non: [x]\nstart: a\nnodes:\n  a: {type: command, run: x, next: ghost}\n  e: {type: end, status: done}\n'
		);
		expect(r.problems.every((p) => p.node === 'a')).toBe(true);
	});
});

describe('serialization', () => {
	it('round-trips the example unchanged', () => {
		const { doc, def } = parseWorkflow(EXAMPLE);
		expect(applyToDocument(doc, def, EXAMPLE)).toBe(EXAMPLE);
	});

	it('keeps comments and untouched nodes when one node changes', () => {
		const { doc, def } = parseWorkflow(EXAMPLE);
		const next: Definition = structuredClone(def);
		next.nodes.find((n) => n.id === 'prepare')!.run = 'mise run setup';
		const out = applyToDocument(doc, next);
		expect(out).toContain('# Example graph workflow');
		expect(out).toContain('# triggers that create a task');
		expect(out).toContain('mise run setup');
		expect(out).toContain(
			'implement: {type: agent, prompt: implement, next: {pushed: open_pr, else: failed}}'
		);
		expect(parseWorkflow(out).def).toEqual(next);
	});

	it('writes reordered conditions, new and deleted nodes', () => {
		const { doc, def } = parseWorkflow(EXAMPLE);
		const next: Definition = structuredClone(def);
		const impl = next.nodes.find((n) => n.id === 'implement')!;
		impl.next = {
			kind: 'cond',
			conds: [
				{ when: 'exit_code == 0', to: 'review' },
				{ when: 'pushed', to: 'open_pr' },
				{ when: 'else', to: 'failed' }
			]
		};
		next.nodes = next.nodes.filter((n) => n.id !== 'prepare');
		next.start = 'implement';
		next.nodes.splice(1, 0, {
			id: 'review',
			type: 'agent',
			prompt: { file: '.enzarb/prompts/review.md' },
			next: { kind: 'to', to: 'open_pr' }
		});
		const out = applyToDocument(doc, next);
		expect(out).not.toContain('prepare');
		const back = parseWorkflow(out).def;
		expect(back).toEqual(next);
		expect(validate(back)).toEqual([]);
	});

	it('serializes a definition from scratch', () => {
		const def: Definition = {
			version: 1,
			name: 'n',
			on: ['issue_opened'],
			start: 'a',
			nodes: [
				{
					id: 'a',
					type: 'wait',
					events: ['comment'],
					from: 'anyone',
					next: { kind: 'to', to: 'e' },
					max_visits: 2,
					on_limit: 'e'
				},
				{ id: 'e', type: 'end', status: 'done' }
			]
		};
		expect(parseWorkflow(serialize(def)).def).toEqual(def);
	});
});
