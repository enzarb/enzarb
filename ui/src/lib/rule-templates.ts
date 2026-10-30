import data from './rule-templates/templates.json';
import { renameMode, type ModeDto, type ModesDoc } from '#lib/modes-types.ts';
import type { WorkflowWire, WorkflowsFileWire } from '#lib/workflows-types.ts';

/**
 * Rule templates: a workflow plus the modes it needs (ez has no built-in modes, ADR-0028 §4).
 * The data lives in `rule-templates/templates.json`; tracks' `rule_templates` test validates every
 * template against the real schema.
 */
export interface RuleTemplate {
	id: string;
	title: string;
	description: string;
	defaultMode: string;
	modes: ModeDto[];
	workflow: WorkflowWire;
}

export const RULE_TEMPLATES: readonly RuleTemplate[] = data as unknown as RuleTemplate[];

const free = (used: Set<string>, base: string) => {
	if (!used.has(base)) return base;
	for (let i = 2; ; i++) if (!used.has(`${base}-${i}`)) return `${base}-${i}`;
};

/**
 * Adds the templates to the current files. A workflow or mode id already taken gets a numeric
 * suffix, and the template's references to it are rewritten. Returns the merged documents and the
 * ids the workflows ended up with.
 */
export function mergeTemplates(
	file: WorkflowsFileWire | null,
	modes: ModesDoc,
	templates: readonly RuleTemplate[]
): { file: WorkflowsFileWire; modes: ModesDoc; workflowIds: string[] } {
	const workflows: Record<string, WorkflowWire> = { ...(file?.workflows ?? {}) };
	let doc: ModesDoc = { defaultMode: modes.defaultMode, modes: [...modes.modes] };
	const workflowIds: string[] = [];
	for (const t of templates) {
		let tm: ModesDoc = {
			defaultMode: t.defaultMode,
			modes: structuredClone(t.modes) as ModeDto[]
		};
		const wf = structuredClone(t.workflow) as WorkflowWire;
		const used = new Set(doc.modes.map((m) => m.id));
		for (let i = 0; i < tm.modes.length; i++) {
			const old = tm.modes[i].id;
			const id = free(used, old);
			used.add(id);
			if (id === old) continue;
			tm = renameMode(tm, i, id);
			for (const n of Object.values(wf.nodes))
				if (n.type === 'agent' && n.mode === old) n.mode = id;
		}
		doc = {
			defaultMode: doc.defaultMode || tm.defaultMode,
			modes: [...doc.modes, ...tm.modes]
		};
		const wid = free(new Set(Object.keys(workflows)), t.id);
		workflows[wid] = wf;
		workflowIds.push(wid);
	}
	return { file: { version: file?.version ?? 1, workflows }, modes: doc, workflowIds };
}
