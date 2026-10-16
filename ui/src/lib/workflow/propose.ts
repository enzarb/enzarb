/**
 * Client-side order of "propose change": branch, commit, pull request (each a remote command).
 * The server chains them with a signed proposal token; the client only passes it along.
 */
/** Workflow names that may go into a branch, commit message and PR title: printable, one line. */
export const WORKFLOW_NAME_RE = /^[A-Za-z0-9][A-Za-z0-9 _.-]{0,63}$/;

export interface ChangedFileInput {
	path: string;
	text: string;
}

export interface ProposeCommands {
	start: (a: { workflow: string }) => Promise<{ branch: string; token: string }>;
	commit: (a: {
		workflow: string;
		token: string;
		files: ChangedFileInput[];
	}) => Promise<{ sha: string; token: string }>;
	open: (a: { workflow: string; token: string }) => Promise<{ number: number; url: string }>;
}

/** Runs the three steps, reporting each as it finishes; throws at the step that failed. */
export async function proposeChange(
	cmds: ProposeCommands,
	files: ChangedFileInput[],
	workflow: string,
	step: (id: 'branch' | 'commit' | 'pr', message?: string) => void
): Promise<{ url: string }> {
	const started = await cmds.start({ workflow });
	step('branch', started.branch);
	const committed = await cmds.commit({ workflow, token: started.token, files });
	step(
		'commit',
		`${committed.sha.slice(0, 7)} · ${files.length} file${files.length === 1 ? '' : 's'}`
	);
	const pr = await cmds.open({ workflow, token: committed.token });
	step('pr', `#${pr.number}`);
	return { url: pr.url };
}
