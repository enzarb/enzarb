export const WORKFLOWS_PATH = '.ez/workflows.toml';
export const MODES_PATH = '.ez/modes.toml';

/** The `file_read` feature name (proto `Feature.FileRead`). */
export const FILE_READ_FEATURE = 'file_read';

/** Proto `Feature.FileReadChat`: relative `file_read` paths resolve against the chat's worktree. */
export const FILE_READ_CHAT_FEATURE = 'file_read_chat';

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
