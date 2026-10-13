import type { Sql } from './db.ts';

/** Presence of the user's Claude token; the UI never sees the token itself (ADR-0004 amendment). */
export interface ClaudeCredential {
	configured: boolean;
	updatedAt: string | null;
}

/** Reads `app_api.claude_credential_v` (owner, present, updated_at) for one GitHub login. */
export async function getClaudeCredential(sql: Sql, login: string): Promise<ClaudeCredential> {
	const [row] = await sql<{ present: boolean; updated_at: Date | null }[]>`
		SELECT present, updated_at FROM app_api.claude_credential_v WHERE owner = ${login}`;
	return { configured: row?.present ?? false, updatedAt: row?.updated_at?.toISOString() ?? null };
}
