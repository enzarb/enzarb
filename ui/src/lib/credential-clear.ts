/** What `clearClaudeToken` needs; injectable for tests. */
export interface ClearDeps {
	connect: (project: string) => Promise<{ credentialClear(): void; close(): unknown }>;
	/** Re-reads `app_api.claude_credential_v`; true while the token is still present. */
	stillConfigured: () => Promise<boolean>;
	wait?: (ms: number) => Promise<void>;
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/**
 * Sends `credential_clear` through one of the user's sandboxes; the proxy deletes the user's row.
 * The frame has no reply, so the view is polled until the row is gone. Returns false when it
 * was still there after the last attempt.
 */
export async function clearClaudeToken(project: string, deps: ClearDeps, attempts = 6) {
	const wait = deps.wait ?? sleep;
	const session = await deps.connect(project);
	try {
		session.credentialClear();
		for (let i = 0; i < attempts; i++) {
			await wait(250 * (i + 1));
			if (!(await deps.stillConfigured())) return true;
		}
		return false;
	} finally {
		void session.close();
	}
}
