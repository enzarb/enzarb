import type { FileReply } from '@enzarb/proto';
import type { StepItem } from '#lib/components/ui/StepList.svelte';
import type { ModesDoc, ModeIssue } from '#lib/modes-types.ts';
import { diffRules, isEmptyDiff, type RulesDiff } from '#lib/rules-diff.ts';
import { MODES_PATH, WORKFLOWS_PATH } from '#lib/rules-chat.ts';
import {
	hasErrorDiagnostics,
	type Diagnostic,
	type WorkflowsFileWire
} from '#lib/workflows-types.ts';

type Result<T> = ({ ok: true } & T) | { ok: false; message: string };

/** What the proposal needs from the sandbox (file reads) and from tracks (validation). */
export interface ProposalDeps {
	readFile: (path: string) => Promise<FileReply>;
	parseModes: (text: string) => Promise<Result<{ doc: ModesDoc; issues: ModeIssue[] }>>;
	checkRules: (
		text: string,
		modesToml: string | null
	) => Promise<Result<{ diagnostics: Diagnostic[]; file: WorkflowsFileWire | null }>>;
}

/** The saved state the agent's edit is compared with. */
export interface Baseline {
	file: WorkflowsFileWire | null;
	modes: ModesDoc | null;
	modesToml: string | null;
}

export interface Proposal {
	diff: RulesDiff;
	file: WorkflowsFileWire;
	rulesText: string | null;
	modes: ModesDoc | null;
	modesText: string | null;
	rulesChanged: boolean;
	modesChanged: boolean;
}

/** Read one file; a missing file is `null`, any other failure an error message. */
async function read(
	deps: ProposalDeps,
	path: string
): Promise<{ ok: true; text: string | null } | { ok: false; message: string }> {
	const r = await deps.readFile(path);
	if (r.ok) return { ok: true, text: r.content };
	if (r.code === 'not_found') return { ok: true, text: null };
	return { ok: false, message: `Could not read ${path}: ${r.message}` };
}

const firstError = (issues: { severity: string; message: string }[]) =>
	issues.find((i) => i.severity === 'error')?.message;

/**
 * Read the agent's two files from the sandbox (relative paths: the sandbox resolves them against
 * the chat's worktree), validate them through tracks and compare them with the saved ones.
 * `proposal: null` means the files do not differ from the saved ones. `key` identifies the content
 * read; when it equals `prevKey` nothing is validated and `same` is true.
 */
export async function buildProposal(
	deps: ProposalDeps,
	saved: Baseline,
	prevKey?: string | null
): Promise<Result<{ proposal: Proposal | null; key: string; same?: boolean }>> {
	const [w, m] = await Promise.all([read(deps, WORKFLOWS_PATH), read(deps, MODES_PATH)]);
	if (!w.ok) return w;
	if (!m.ok) return m;
	const key = JSON.stringify([w.text, m.text]);
	if (key === prevKey) return { ok: true, proposal: null, key, same: true };

	let modes = saved.modes;
	let modesToml = saved.modesToml;
	if (m.text !== null) {
		const p = await deps.parseModes(m.text);
		if (!p.ok) return { ok: false, message: `${MODES_PATH}: ${p.message}` };
		const bad = firstError(p.issues);
		if (bad) return { ok: false, message: `${MODES_PATH} has an error: ${bad}` };
		modes = p.doc;
		modesToml = m.text;
	}

	let file: WorkflowsFileWire = saved.file ?? { version: 1 };
	if (w.text !== null) {
		const c = await deps.checkRules(w.text, modesToml);
		if (!c.ok) return { ok: false, message: `${WORKFLOWS_PATH}: ${c.message}` };
		if (hasErrorDiagnostics(c.diagnostics) || !c.file) {
			return {
				ok: false,
				message: `${WORKFLOWS_PATH} has an error: ${firstError(c.diagnostics) ?? 'it does not parse'}`
			};
		}
		file = c.file;
	}

	const diff = diffRules(saved.file, file, saved.modes, modes);
	if (isEmptyDiff(diff)) return { ok: true, proposal: null, key };
	return {
		ok: true,
		key,
		proposal: {
			diff,
			file,
			rulesText: w.text,
			modes,
			modesText: modesToml,
			rulesChanged:
				diff.addedRules.length + diff.removedRules.length + diff.changedRules.length > 0,
			modesChanged: diff.addedModes.length + diff.removedModes.length + diff.changedModes.length > 0
		}
	};
}

/** The checklist an accept walks through; modes are committed first (workflows refer to them). */
export function acceptSteps(p: Proposal, branch: string, rulesPath: string): StepItem[] {
	const items: StepItem[] = [{ id: 'check', status: 'active', label: 'Check the changes' }];
	if (p.modesChanged) {
		items.push({ id: 'modes', status: 'pending', label: `Commit ${MODES_PATH} to ${branch}` });
	}
	if (p.rulesChanged) {
		items.push({ id: 'rules', status: 'pending', label: `Commit ${rulesPath} to ${branch}` });
	}
	return items;
}
