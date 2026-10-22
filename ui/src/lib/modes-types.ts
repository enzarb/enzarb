/**
 * `.ez/modes.toml` as enzarb-tracks returns it (ez-core `ModesDoc`, camelCase). The designer edits
 * this document; tracks validates it and writes it back to TOML (ADR-0028, ADR-0029).
 */

export interface HookDto {
	run: string;
	glob?: string | null;
	matcher?: string | null;
	/** `ignore`, `block` or `fix_turn`. */
	onFailure: string;
	maxFixTurns: number;
	timeoutSecs: number;
}

export interface ModeDto {
	id: string;
	/** The id in the loaded file; differs from `id` when the mode is renamed. Absent for new modes. */
	originalId?: string;
	name?: string | null;
	description: string;
	prompt: string;
	/** `null` means every built-in tool. */
	tools: string[] | null;
	miseTasks: string[];
	mcpTools: string[];
	next: string[];
	autoAdvanceTo: string[];
	profile?: string | null;
	model?: string | null;
	/** Tool name or glob to `allow` / `ask` / `deny`. */
	permissions: Record<string, string>;
	/** Hooks per event (`session_start`, `post_edit`, ...). */
	hooks: Record<string, HookDto[]>;
}

export interface ModesDoc {
	defaultMode: string;
	modes: ModeDto[];
}

export type IssueSeverity = 'error' | 'warning' | 'info';

/** A validation finding; `path` is `defaultMode`, `modes[2].tools[1]`, `modes[0].hooks.post_edit[0].run`, ... */
export interface ModeIssue {
	path: string;
	message: string;
	severity: IssueSeverity;
}

export interface ModesCatalog {
	builtinTools: { name: string; kind: string; readOnly: boolean }[];
	hookEvents: { name: string; glob: boolean; matcher: boolean; description: string }[];
	permissionValues: string[];
	onFailureValues: string[];
	/** `ez init` templates: `full` (the six lifecycle modes) or `minimal`. */
	templates: { set: string; mode: ModeDto }[];
}

/** One repository's `.ez/modes.toml` as the designer opens it. */
export interface ModesFile {
	/** `github:owner/repo`. */
	repo: string;
	/** The default branch: read from and committed to. */
	branch: string;
	path: string;
	/** Blob sha of the loaded file (null: no file yet); sent back on save. */
	sha: string | null;
	text: string | null;
	doc: ModesDoc;
	issues: ModeIssue[];
	/** The file exists but is not valid modes TOML: only the TOML editor can fix it. */
	parseError: string | null;
	catalog: ModesCatalog;
}

/** Whether any issue blocks saving. */
export const hasErrors = (issues: readonly ModeIssue[]) =>
	issues.some((i) => i.severity === 'error');

/** Index of the mode an issue path points into (`modes[3]...` -> 3), or null. */
export function issueModeIndex(path: string): number | null {
	const m = /^modes\[(\d+)\]/.exec(path);
	return m ? Number(m[1]) : null;
}

/** A blank mode with ez's defaults (every built-in tool). */
export function blankMode(id: string): ModeDto {
	return {
		id,
		name: null,
		description: '',
		prompt: '',
		tools: null,
		miseTasks: [],
		mcpTools: [],
		next: [],
		autoAdvanceTo: [],
		profile: null,
		model: null,
		permissions: {},
		hooks: {}
	};
}

/** A mode id not yet used in `doc`, starting from `base`. */
export function freeModeId(doc: ModesDoc, base = 'mode'): string {
	const used = new Set(doc.modes.map((m) => m.id));
	if (!used.has(base)) return base;
	for (let i = 2; ; i++) if (!used.has(`${base}-${i}`)) return `${base}-${i}`;
}

/**
 * Rename a mode and every reference to it (`defaultMode`, `next`, `autoAdvanceTo`), keeping the
 * on-disk id in `originalId` so tracks rewrites the section header. Mirrors ez-core `rename_mode`.
 */
export function renameMode(doc: ModesDoc, index: number, id: string): ModesDoc {
	const old = doc.modes[index]?.id;
	if (old === undefined || old === id) return doc;
	const swap = (list: string[]) => list.map((r) => (r === old ? id : r));
	return {
		defaultMode: doc.defaultMode === old ? id : doc.defaultMode,
		modes: doc.modes.map((m, i) => ({
			...m,
			...(i === index ? { id } : {}),
			next: swap(m.next),
			autoAdvanceTo: swap(m.autoAdvanceTo)
		}))
	};
}

/** Remove a mode and every reference to it; the default moves to the first remaining mode. */
export function removeMode(doc: ModesDoc, index: number): ModesDoc {
	const gone = doc.modes[index]?.id;
	if (gone === undefined) return doc;
	const modes = doc.modes
		.filter((_, i) => i !== index)
		.map((m) => ({
			...m,
			next: m.next.filter((r) => r !== gone),
			autoAdvanceTo: m.autoAdvanceTo.filter((r) => r !== gone)
		}));
	const defaultMode = doc.defaultMode === gone ? (modes[0]?.id ?? '') : doc.defaultMode;
	return { defaultMode, modes };
}

/** Messages of the issues at `prefix` or below it (`modes[1].tools` matches `modes[1].tools[0]`). */
export function issuesAt(issues: readonly ModeIssue[], prefix: string): string[] {
	return issues
		.filter(
			(i) => i.path === prefix || i.path.startsWith(`${prefix}.`) || i.path.startsWith(`${prefix}[`)
		)
		.map((i) => i.message);
}
