import { describe, expect, it } from 'vitest';
import { mergeTemplates, RULE_TEMPLATES } from './rule-templates.ts';

describe('rule templates', () => {
	it('each agent node uses a mode the template brings', () => {
		expect(RULE_TEMPLATES.length).toBeGreaterThanOrEqual(3);
		for (const t of RULE_TEMPLATES) {
			const ids = t.modes.map((m) => m.id);
			for (const n of Object.values(t.workflow.nodes))
				if (n.type === 'agent') expect(ids).toContain(n.mode);
		}
	});

	it('suffixes colliding workflow and mode ids and rewrites references', () => {
		const t = RULE_TEMPLATES[0];
		const once = mergeTemplates(null, { defaultMode: '', modes: [] }, [t]);
		const twice = mergeTemplates(once.file, once.modes, [t]);
		expect(twice.workflowIds).toEqual([`${t.id}-2`]);
		expect(twice.modes.modes.map((m) => m.id)).toEqual([t.modes[0].id, `${t.modes[0].id}-2`]);
		const node = Object.values(twice.file.workflows![`${t.id}-2`].nodes).find(
			(n) => n.type === 'agent'
		);
		expect(node?.type === 'agent' && node.mode).toBe(`${t.modes[0].id}-2`);
		expect(twice.modes.defaultMode).toBe(t.defaultMode);
	});
});
