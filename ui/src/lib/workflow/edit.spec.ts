import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { parseWorkflow, validate } from './definition.ts';
import { connect, deleteNode, drawnEdges, freeId, layout, renameNode, retype } from './edit.ts';
import { EXAMPLE_ISSUE } from './example.ts';

const ex = () => parseWorkflow(EXAMPLE_ISSUE).def;

describe('editing', () => {
	it('the embedded example equals docs/workflows/examples/issue.yaml', () => {
		const docs = new URL('../../../../docs/workflows/examples/issue.yaml', import.meta.url);
		expect(EXAMPLE_ISSUE).toBe(readFileSync(docs, 'utf8'));
	});

	it('renames a node and its references', () => {
		const d = ex();
		renameNode(d, 'prepare', 'setup');
		renameNode(d, 'failed', 'oops');
		expect(d.start).toBe('setup');
		expect(d.nodes.find((n) => n.id === 'implement')!.next).toEqual({
			kind: 'cond',
			conds: [
				{ when: 'pushed', to: 'open_pr' },
				{ when: 'else', to: 'oops' }
			]
		});
		expect(validate(d)).toEqual([]);
	});

	it('deletes a node and the edges to it', () => {
		const d = ex();
		deleteNode(d, 'open_pr');
		expect(d.nodes.map((n) => n.id)).not.toContain('open_pr');
		expect(d.nodes.find((n) => n.id === 'implement')!.next).toEqual({
			kind: 'cond',
			conds: [{ when: 'else', to: 'failed' }]
		});
	});

	it('connects: plain next first, then a condition before else', () => {
		const d = ex();
		d.nodes.push({ id: 'x', type: 'command', run: 'y' });
		connect(d, 'x', 'done');
		expect(d.nodes.at(-1)!.next).toEqual({ kind: 'to', to: 'done' });
		connect(d, 'x', 'failed');
		expect(d.nodes.at(-1)!.next).toEqual({
			kind: 'cond',
			conds: [
				{ when: 'ok', to: 'failed' },
				{ when: 'else', to: 'done' }
			]
		});
		connect(d, 'done', 'x');
		expect(d.nodes.find((n) => n.id === 'done')!.next).toBeUndefined();
	});

	it('retypes, keeping edges but not foreign fields', () => {
		const n = retype(
			{ id: 'a', type: 'command', run: 'x', optional: true, next: { kind: 'to', to: 'e' } },
			'wait'
		);
		expect(n).toEqual({
			id: 'a',
			type: 'wait',
			events: ['comment'],
			next: { kind: 'to', to: 'e' },
			max_visits: undefined,
			on_limit: undefined,
			optional: undefined
		});
	});

	it('finds free ids', () => {
		expect(freeId(ex(), 'done')).toBe('done_2');
		expect(freeId(ex(), 'new')).toBe('new');
	});

	it('draws labelled edges in condition order and lays out every node', () => {
		const d = ex();
		expect(drawnEdges(d).filter((e) => e.source === 'implement')).toMatchObject([
			{ target: 'open_pr', label: '1. pushed', kind: 'cond' },
			{ target: 'failed', label: 'else', kind: 'else' }
		]);
		const pos = layout(d);
		expect(pos.size).toBe(5);
		expect(pos.get('prepare')!.y).toBeLessThan(pos.get('implement')!.y);
	});
});
