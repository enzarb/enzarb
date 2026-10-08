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
		},
		/**
		 * Records the sandbox's own answer. `true` keeps an earlier who/when; `false` clears the flag
		 * (no row is created for a project that never had one).
		 */
		async recordClaudeState(project: string, configured: boolean, login: string): Promise<void> {
			if (configured) {
				await sql`INSERT INTO app.project_settings
					(project, claude_token_configured, claude_configured_at, claude_configured_by)
					VALUES (${project}, true, now(), ${login})
					ON CONFLICT (project) DO UPDATE SET
						claude_configured_at = CASE WHEN app.project_settings.claude_token_configured
							THEN app.project_settings.claude_configured_at ELSE now() END,
						claude_configured_by = CASE WHEN app.project_settings.claude_token_configured
							THEN app.project_settings.claude_configured_by ELSE ${login} END,
						claude_token_configured = true, updated_at = now()`;
			} else {
				await sql`UPDATE app.project_settings SET claude_token_configured = false,
					claude_configured_at = NULL, claude_configured_by = NULL, updated_at = now()
					WHERE project = ${project} AND claude_token_configured`;
			}
		}
	};
}
