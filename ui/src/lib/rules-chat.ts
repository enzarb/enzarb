import exampleWorkflows from './rule-templates/chat-example.workflows.toml?raw';
import exampleModes from './rule-templates/chat-example.modes.toml?raw';

export const WORKFLOWS_PATH = '.ez/workflows.toml';
export const MODES_PATH = '.ez/modes.toml';

/** The `file_read` feature name (proto `Feature.FileRead`). */
export const FILE_READ_FEATURE = 'file_read';

/** Proto `Feature.FileReadChat`: relative `file_read` paths resolve against the chat's worktree. */
export const FILE_READ_CHAT_FEATURE = 'file_read_chat';

/** Markers around the context in the first message; the transcript hides what is between them. */
export const CONTEXT_START = '<enzarb-context>';
export const CONTEXT_END = '</enzarb-context>';

/** Splits a message into the hidden context block and the user's own text; null when it has none. */
export function splitContext(text: string): { context: string; rest: string } | null {
	if (!text.startsWith(CONTEXT_START)) return null;
	const end = text.indexOf(CONTEXT_END);
	if (end < 0) return null;
	return {
		context: text.slice(CONTEXT_START.length, end).trim(),
		rest: text.slice(end + CONTEXT_END.length).trim()
	};
}

/** Compact reference of the two file formats, with one valid example pair. */
export const FORMAT_REFERENCE = [
	'File formats (TOML). Unknown keys are errors. Maps, not arrays: never write [[...]].',
	'',
	'.ez/workflows.toml',
	'- `version = 1` at the top.',
	'- `[workflows.<id>]`: `description`, `on` (list of events, e.g. "pull_request.opened", "issues.labeled", "push"), optional `if` (condition on the event), `start` (a node id), optional `timeout`, `max_runs`.',
	'- Nodes are a MAP: `[workflows.<id>.nodes.<node-id>]`, each with `type`:',
	'  - agent: `mode` (a mode id from modes.toml), `instruction` (template, e.g. {{ event.pull_request.number }}), optional `result` (map of name to JSON-schema, e.g. { summary = { type = "string" } }), `push` (bool), `next`.',
	'  - command: `task` (mise task) with `args`, or `run` (shell command); `next`.',
	'  - action: `action` = comment | open_pull_request | add_labels | remove_labels; `body`, `title`, `labels`, `draft`; `next`. Output of an earlier node: {{ nodes.<node-id>.result.<name> }}.',
	'  - wait: `on` (events), optional `if`, `timeout`; `next`.',
	'  - end: `status` = done | failed | cancelled (no `next`).',
	'- `next` is a node id ("comment") or ordered edges: `next = [{ if = "...", to = "a" }, { to = "b" }]`. Every path must reach an end node.',
	'',
	'.ez/modes.toml',
	'- `default_mode = "<id>"` (must be defined).',
	'- `[modes.<id>]`: `description`, `prompt` (instructions for the mode), `tools` (allowlist of built-ins: Read, Write, Edit, Glob, Grep, Bash, TodoWrite; omit for all), optional `model`, `mise_tasks`, `next`.',
	'',
	'Example .ez/workflows.toml:',
	'```toml',
	exampleWorkflows.trimEnd(),
	'```',
	'Example .ez/modes.toml:',
	'```toml',
	exampleModes.trimEnd(),
	'```'
].join('\n');

/** The first message of a chat about rules: vocabulary, the current files, and the ground rules. */
export function rulesContextMessage(
	workflowsToml: string | null,
	modesToml: string | null
): string {
	const block = (name: string, text: string | null) =>
		`${name}:\n\`\`\`toml\n${text?.trim() ? text.trimEnd() : '# (does not exist yet)'}\n\`\`\``;
	return [
		CONTEXT_START,
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
		'Keep every mode a workflow refers to defined in modes.toml. Explain briefly what you changed.',
		'',
		FORMAT_REFERENCE,
		CONTEXT_END
	].join('\n');
}

/** What the agent is asked after a discard. */
export const REVERT_MESSAGE =
	`The user discarded your proposal. Revert your edits to \`${WORKFLOWS_PATH}\` and \`${MODES_PATH}\` ` +
	'(restore both files to how they were before), and do not make them again unless asked.';

/** The message that asks the agent to fix files that failed validation. */
export function fixMessage(errors: string): string {
	return `Your last edit to the rules files is invalid. Please fix it and keep to the file formats above:\n${errors}`;
}

/** Automatic fix requests allowed in a row without input from the user. */
export const MAX_AUTO_FIXES = 2;

/**
 * Decides whether to ask the agent to fix an error by itself: once per distinct error text, at
 * most MAX_AUTO_FIXES in a row (the user's own message or a valid result resets the count).
 */
export function createAutoFix() {
	let count = 0;
	let last = '';
	return {
		/** Returns the message to send, or null. Call when a turn ended with the files still invalid. */
		next(errors: string): string | null {
			if (!errors || errors === last || count >= MAX_AUTO_FIXES) return null;
			last = errors;
			count++;
			return fixMessage(errors);
		},
		/** The user typed something, or the files became valid. */
		reset() {
			count = 0;
			last = '';
		}
	};
}
