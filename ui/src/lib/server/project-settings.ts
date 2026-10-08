import type { Sql } from './db.ts';

export interface ProjectSettings {
	claudeTokenConfigured: boolean;
	claudeConfiguredAt: string | null;
	claudeConfiguredBy: string | null;
}

export const DEFAULT_SETTINGS: ProjectSettings = {
	claudeTokenConfigured: false,
	claudeConfiguredAt: null,
	claudeConfiguredBy: null
};

/** app.project_settings access. Callers authorize the project before calling. */
export function projectSettings(sql: Sql) {
	return {
		async get(project: string): Promise<ProjectSettings> {
			const [row] = await sql<
				{ configured: boolean; at: Date | null; by: string | null }[]
			>`SELECT claude_token_configured AS configured, claude_configured_at AS at, claude_configured_by AS by
			  FROM app.project_settings WHERE project = ${project}`;
			if (!row) return DEFAULT_SETTINGS;
			return {
				claudeTokenConfigured: row.configured,
				claudeConfiguredAt: row.at?.toISOString() ?? null,
				claudeConfiguredBy: row.by
			};
		},
		async markClaudeConfigured(project: string, login: string): Promise<void> {
			await sql`INSERT INTO app.project_settings
				(project, claude_token_configured, claude_configured_at, claude_configured_by)
				VALUES (${project}, true, now(), ${login})
				ON CONFLICT (project) DO UPDATE SET claude_token_configured = true,
					claude_configured_at = now(), claude_configured_by = ${login}, updated_at = now()`;
		}
	};
}
