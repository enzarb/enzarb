/** Client-side order of "propose change": branch, commit, pull request (each a remote command). */
export interface ChangedFileInput {
	path: string;
	text: string;
}

export interface ProposeCommands {
	start: (a: { workflow: string }) => Promise<{ branch: string; base: string }>;
	commit: (a: { workflow: string; branch: string; files: ChangedFileInput[] }) => Promise<string>;
	open: (a: {
		workflow: string;
		branch: string;
		paths: string[];
	}) => Promise<{ number: number; url: string }>;
}

/** Runs the three steps, reporting each as it finishes; throws at the step that failed. */
export async function proposeChange(
	cmds: ProposeCommands,
	files: ChangedFileInput[],
	workflow: string,
	step: (id: 'branch' | 'commit' | 'pr', message?: string) => void
): Promise<{ url: string }> {
	const { branch } = await cmds.start({ workflow });
	step('branch', branch);
	const sha = await cmds.commit({ workflow, branch, files });
	step('commit', `${sha.slice(0, 7)} · ${files.length} file${files.length === 1 ? '' : 's'}`);
	const pr = await cmds.open({ workflow, branch, paths: files.map((f) => f.path) });
	step('pr', `#${pr.number}`);
	return { url: pr.url };
}
