import type { ModesDoc } from '#lib/modes-types.ts';
import { eventLabel } from '#lib/rules.ts';
import type { WorkflowWire, WorkflowsFileWire } from '#lib/workflows-types.ts';

/** What an agent's edit changes, per rule (workflow) and mode. Ids are the file's keys. */
export interface RulesDiff {
	addedRules: string[];
	removedRules: string[];
	changedRules: string[];
	addedModes: string[];
	removedModes: string[];
	changedModes: string[];
	/** Plain-language lines, one per change, rules first. */
	summary: string[];
}

const same = (a: unknown, b: unknown) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

const events = (on: string[]) => on.map(eventLabel).join(' or ') || 'nothing';

/** The agent nodes' distinct modes, in node order. */
function modesOf(wf: WorkflowWire): string[] {
	const m = Object.values(wf.nodes ?? {}).flatMap((n) =>
		n.type === 'agent' && n.mode ? [n.mode] : []
	);
	return [...new Set(m)];
}

function describeNew(id: string, wf: WorkflowWire): string {
	const cond = wf.if?.trim() ? ` and ${wf.if.trim()}` : '';
	const using = modesOf(wf);
	return (
		`Added the rule "${id}": when ${events(wf.on)}${cond}, run ${wf.description || id}` +
		(using.length ? ` using mode ${using.join(', ')}` : '')
	);
}

function describeChange(id: string, a: WorkflowWire, b: WorkflowWire): string {
	const parts: string[] = [];
	if (!same(a.on, b.on)) parts.push(`it now runs when ${events(b.on)} (was ${events(a.on)})`);
	if ((a.if ?? '').trim() !== (b.if ?? '').trim()) {
		parts.push(b.if?.trim() ? `its condition is now ${b.if.trim()}` : 'its condition was removed');
	}
	const [ma, mb] = [modesOf(a), modesOf(b)];
	if (!same(ma, mb)) parts.push(`it now uses mode ${mb.join(', ') || 'none'}`);
	if (!same(a.nodes, b.nodes) || a.start !== b.start) parts.push('its steps changed');
	if (!parts.length) parts.push('its settings changed');
	return `Changed the rule "${id}": ${parts.join(', ')}`;
}

const stripId = <T extends { originalId?: string }>(m: T) => {
	const { originalId, ...rest } = m;
	void originalId;
	return rest;
};

/** Compare the saved files with the agent's version. A missing `before` is an empty file. */
export function diffRules(
	before: WorkflowsFileWire | null,
	after: WorkflowsFileWire,
	beforeModes: ModesDoc | null,
	afterModes: ModesDoc | null
): RulesDiff {
	const a = before?.workflows ?? {};
	const b = after.workflows ?? {};
	const addedRules = Object.keys(b).filter((k) => !(k in a));
	const removedRules = Object.keys(a).filter((k) => !(k in b));
	const changedRules = Object.keys(b).filter((k) => k in a && !same(a[k], b[k]));

	const ma = new Map((beforeModes?.modes ?? []).map((m) => [m.id, stripId(m)]));
	const mb = new Map((afterModes?.modes ?? []).map((m) => [m.id, stripId(m)]));
	const addedModes = [...mb.keys()].filter((k) => !ma.has(k));
	const removedModes = [...ma.keys()].filter((k) => !mb.has(k));
	const changedModes = [...mb.keys()].filter((k) => ma.has(k) && !same(ma.get(k), mb.get(k)));

	const summary = [
		...addedRules.map((k) => describeNew(k, b[k])),
		...changedRules.map((k) => describeChange(k, a[k], b[k])),
		...removedRules.map((k) => `Removed the rule "${k}"`),
		...addedModes.map((k) => `Added the mode "${k}"`),
		...changedModes.map((k) => `Changed the mode "${k}"`),
		...removedModes.map((k) => `Removed the mode "${k}"`)
	];
	return {
		addedRules,
		removedRules,
		changedRules,
		addedModes,
		removedModes,
		changedModes,
		summary
	};
}

export const isEmptyDiff = (d: RulesDiff) => d.summary.length === 0;

/** Ids of the rules a row should highlight (added or changed). */
export const highlightedRules = (d: RulesDiff | null): Set<string> =>
	new Set(d ? [...d.addedRules, ...d.changedRules] : []);
