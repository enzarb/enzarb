import { describe, expect, it } from 'vitest';
import {
	assumeTemplate,
	blankNode,
	blankWorkflow,
	diagnosticTarget,
	freeId,
	fromWire,
	layoutWorkflow,
	removeNode,
	renameNode,
	toWire,
	type WorkflowsFileWire
} from './workflows-types.ts';

const wire: WorkflowsFileWire = {
	version: 1,
	workflows: {
		fix: {
			description: 'Fix issues',
			on: ['issues.labeled'],
			if: "event.label.name == 'ez'",
			start: 'triage',
			nodes: {
				triage: {
					type: 'agent',
					mode: 'triage',
					instruction: 'Look',
					result: { decision: { enum: ['fix', 'ask'] } },
					next: [{ if: "result.decision == 'fix'", to: 'run' }, { to: 'done' }]
				},
				run: { type: 'command', task: 'test', args: ['--all'], next: 'done', max_visits: 2 },
				done: { type: 'end', status: 'done' }
			}
		}
	}
};

describe('wire conversion', () => {
	it('round-trips a file, keeping order and omitting unset fields', () => {
		const doc = fromWire(wire);
		expect(doc.workflows[0].nodes.map((n) => n.id)).toEqual(['triage', 'run', 'done']);
		expect(doc.workflows[0].nodes[1]).toMatchObject({
			type: 'command',
			next: [{ if: '', to: 'done' }],
			max_visits: 2
		});
		expect(toWire(doc)).toEqual(wire);
	});

	it('writes no nulls for lists and booleans serde would reject', () => {
		const wf = blankWorkflow('w');
		wf.nodes[0] = { ...blankNode('start', 'action'), next: [{ if: '', to: 'done' }] };
		const out = toWire({ workflows: [wf] }).workflows!.w;
		expect(out).toEqual({
			on: ['issues.labeled'],
			start: 'start',
			nodes: {
				start: { type: 'action', action: 'comment', next: 'done' },
				done: { type: 'end', status: 'done' }
			}
		});
	});

	it('keeps the fields of other types when the type changes back', () => {
		const n = { ...blankNode('a', 'agent'), mode: 'plan', instruction: 'x', next: [] };
		const asEnd = toWire({
			workflows: [{ ...blankWorkflow('w'), nodes: [{ ...n, type: 'end' }] }]
		});
		expect(asEnd.workflows!.w.nodes.a).toEqual({ type: 'end', status: 'done' });
		expect(
			toWire({ workflows: [{ ...blankWorkflow('w'), nodes: [n] }] }).workflows!.w.nodes.a
		).toMatchObject({ type: 'agent', mode: 'plan', next: [] });
	});
});

describe('editing helpers', () => {
	it('renames a node with its references and removes edges into a removed node', () => {
		const wf = fromWire(wire).workflows[0];
		const r = renameNode(wf, 'triage', 'look');
		expect(r.start).toBe('look');
		expect(r.nodes[0].id).toBe('look');
		const d = renameNode(wf, 'done', 'end');
		expect(d.nodes[0].next.map((e) => e.to)).toEqual(['run', 'end']);
		const x = removeNode(wf, 'run');
		expect(x.nodes.map((n) => n.id)).toEqual(['triage', 'done']);
		expect(x.nodes[0].next.map((e) => e.to)).toEqual(['done']);
	});

	it('finds free ids and diagnostic targets', () => {
		expect(freeId(['a', 'a-2'], 'a')).toBe('a-3');
		expect(freeId(['a'], 'b')).toBe('b');
		expect(diagnosticTarget('workflows.fix.nodes.triage.next[0].if')).toEqual({
			workflow: 'fix',
			node: 'triage'
		});
		expect(diagnosticTarget('workflows.fix.on[0]')).toEqual({ workflow: 'fix', node: null });
		expect(diagnosticTarget('')).toBeNull();
	});

	it('builds assume templates from the result schema', () => {
		const n = fromWire(wire).workflows[0].nodes[0];
		expect(assumeTemplate(n, 'agent')).toMatchObject({
			result: { decision: 'fix' },
			git: { pushed: true }
		});
		expect(assumeTemplate(undefined, 'wait')).toEqual({ event: {}, timed_out: false });
	});
});

describe('layoutWorkflow', () => {
	it('layers by distance from start, marks loops, dangling edges and orphans', () => {
		const wf = fromWire(wire).workflows[0];
		wf.nodes[1].next = [
			{ if: 'exit_code != 0', to: 'triage' },
			{ if: '', to: 'done' },
			{ if: '', to: 'ghost' }
		];
		wf.nodes.push(blankNode('lonely', 'end'));
		const l = layoutWorkflow(wf);
		const at = (id: string) => l.nodes.find((n) => n.id === id)!;
		expect([at('triage').layer, at('run').layer, at('done').layer]).toEqual([0, 1, 1]);
		expect(at('lonely')).toMatchObject({ layer: 2, orphan: true });
		const loop = l.edges.find((e) => e.from === 'run' && e.to === 'triage')!;
		expect(loop).toMatchObject({ back: true, label: 'exit_code != 0' });
		expect(l.edges.find((e) => e.to === 'ghost')).toMatchObject({ dangling: true, label: 'else' });
		expect(l.width).toBeGreaterThan(0);
	});
});
