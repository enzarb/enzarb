import type { ChatItem } from '#lib/acp/types.ts';

export const WORKFLOWS_PATH = '.ez/workflows.toml';
export const MODES_PATH = '.ez/modes.toml';

/** The `file_read` feature name (proto `Feature.FileRead`). */
export const FILE_READ_FEATURE = 'file_read';

const EZ_FILE = /((?:\/[\w.@+-]+)*\/)?\.ez\/(?:workflows|modes)\.toml/;

/**
 * The directory prefix of an edit to `.ez/workflows.toml` or `.ez/modes.toml` in a completed tool
 * call (`'/workspace/worktrees/chat-1/'`, or `''` for a relative path), else null. The chat runs in
 * its own worktree whose name the browser does not know, so the tool call names it.
 */
export function ezEditBase(item: ChatItem): string | null {
	if (item.kind !== 'tool' || item.status !== 'completed') return null;
	if (item.toolKind === 'read' || item.toolKind === 'search' || item.toolKind === 'fetch') {
		return null;
	}
	const m = EZ_FILE.exec(`${item.title}\n${item.text}`);
	return m ? (m[1] ?? '') : null;
}

/** The first message of a chat about rules: vocabulary, the current files, and the ground rules. */
export function rulesContextMessage(
	workflowsToml: string | null,
	modesToml: string | null
): string {
	const block = (name: string, text: string | null) =>
		`${name}:\n\`\`\`toml\n${text?.trim() ? text.trimEnd() : '# (does not exist yet)'}\n\`\`\``;
	return [
		'You are helping the user change the automation rules of this repository.',
		'A rule reads: When <events> and <condition> Then run <workflow> Using mode <mode>.',
		"- When: GitHub events such as issues.labeled or push, in the workflow's `on` list.",
		'- and: an optional `if` condition on the event payload.',
		'- Then run: a workflow in `.ez/workflows.toml` (nodes and edges).',
		"- Using mode: the `mode` of the workflow's agent nodes, defined in `.ez/modes.toml`.",
		'',
		block('Current .ez/workflows.toml', workflowsToml),
		block('Current .ez/modes.toml', modesToml),
		'',
		`Make changes by editing \`${WORKFLOWS_PATH}\` and \`${MODES_PATH}\` in your working directory under /workspace.`,
		'Never commit, push, or run git write commands: the user reviews your edits as a proposal and enzarb commits them if they accept.',
		'Keep every mode a workflow refers to defined in modes.toml. Explain briefly what you changed.'
	].join('\n');
}

/** What the agent is asked after a discard. */
export const REVERT_MESSAGE =
	`The user discarded your proposal. Revert your edits to \`${WORKFLOWS_PATH}\` and \`${MODES_PATH}\` ` +
	'(restore both files to how they were before), and do not make them again unless asked.';
