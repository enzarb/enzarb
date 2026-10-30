import { describe, expect, it } from 'vitest';
import type { ModesDoc } from '#lib/modes-types.ts';
import { rulesContextMessage } from '#lib/rules-chat.ts';
import { diffRules, highlightedRules, isEmptyDiff } from '#lib/rules-diff.ts';
import { exampleWorkflow, toWire } from '#lib/workflows-types.ts';

const before = toWire({ workflows: [exampleWorkflow('fix-issue')] });
const clone = <T>(v: T): T => structuredClone(v);
const mode = (id: string, prompt = 'p') => ({
	id,
	description: '',
	prompt,
	tools: null,
	miseTasks: [],
	mcpTools: [],
	next: [],
	autoAdvanceTo: [],
	permissions: {},
	hooks: {}
});
const modes = (...m: ReturnType<typeof mode>[]): ModesDoc => ({ defaultMode: 'a', modes: m });

describe('diffRules', () => {
	it('is empty for identical files', () => {
		const d = diffRules(before, clone(before), modes(mode('a')), modes(mode('a')));
		expect(isEmptyDiff(d)).toBe(true);
		expect(highlightedRules(d).size).toBe(0);
	});

	it('summarises an added rule with eventLabel and its modes', () => {
		const after = clone(before);
		after.workflows!['review'] = { ...clone(after.workflows!['fix-issue']), on: ['push'], if: '' };
		const d = diffRules(before, after, null, null);
		expect(d.addedRules).toEqual(['review']);
		expect(d.summary[0]).toContain('Added the rule "review": when code is pushed');
		expect(d.summary[0]).toContain('using mode');
		expect([...highlightedRules(d)]).toEqual(['review']);
	});

	it('summarises changed events and removed rules', () => {
		const after = clone(before);
		after.workflows!['fix-issue'].on = ['pull_request.opened'];
		const d = diffRules(before, after, null, null);
		expect(d.changedRules).toEqual(['fix-issue']);
		expect(d.summary[0]).toBe(
			'Changed the rule "fix-issue": it now runs when a pull request is opened (was an issue is labeled)'
		);
		const gone = diffRules(before, { version: 1 }, null, null);
		expect(gone.summary).toEqual(['Removed the rule "fix-issue"']);
		expect(highlightedRules(gone).size).toBe(0);
	});

	it('reports a removed condition', () => {
		const after = clone(before);
		after.workflows!['fix-issue'].if = '';
		const d = diffRules(before, after, null, null);
		expect(d.summary[0]).toContain('its condition was removed');
	});

	it('reports added and changed modes, ignoring originalId', () => {
		const old = modes({ ...mode('a'), originalId: 'a' } as ReturnType<typeof mode>);
		const next = modes(mode('a', 'new prompt'), mode('b'));
		const d = diffRules(null, { version: 1 }, old, next);
		expect(d.addedModes).toEqual(['b']);
		expect(d.changedModes).toEqual(['a']);
		const same = diffRules(null, { version: 1 }, old, modes(mode('a')));
		expect(isEmptyDiff(same)).toBe(true);
	});
});

describe('rules chat helpers', () => {
	it('the context message carries the vocabulary, both files and the never-commit rule', () => {
		const m = rulesContextMessage('version = 1', null);
		expect(m).toContain('When <events>');
		expect(m).toContain('version = 1');
		expect(m).toContain('(does not exist yet)');
		expect(m).toContain('.ez/workflows.toml');
		expect(m).toContain('Never commit');
	});
});
