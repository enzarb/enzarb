/** Largest prompt (UTF-8 bytes) the sandbox accepts. */
export const MAX_PROMPT_BYTES = 256 * 1024;
/** Most in-flight JSON-RPC requests the sandbox tolerates. */
export const MAX_PENDING = 64;

export type PermissionStatus = 'pending' | 'allowed' | 'denied' | 'cancelled' | 'blocked';
/** The only option kinds the UI renders; `allow_always` / `reject_always` are dropped. */
export type PermissionOptionKind = 'allow_once' | 'reject_once';
export type PermissionChoice = 'allow' | 'deny';

export interface PermissionOption {
	optionId: string;
	name: string;
	kind: PermissionOptionKind;
}

/** Plain-text view of a tool call; everything is a string so it is rendered by interpolation only. */
export interface ToolView {
	toolCallId: string;
	title: string;
	toolKind?: string;
	status?: string;
	text: string;
}

export interface PlanEntry {
	content: string;
	status: string;
	priority: string;
}

export type TextRole = 'user' | 'agent' | 'thought';

export type ChatItem =
	| { kind: TextRole; id: string; text: string; messageId?: string }
	| ({ kind: 'tool'; id: string } & ToolView)
	| { kind: 'plan'; id: string; entries: PlanEntry[] }
	| {
			kind: 'permission';
			id: string;
			toolCall: ToolView;
			options: PermissionOption[];
			status: PermissionStatus;
			/** Untrusted display text only; never changes the choices offered. */
			hint?: string;
			/** Why the sandbox blocked the action (`blocked` items). */
			reason?: string;
	  }
	| {
			kind: 'question';
			id: string;
			questions: QuestionView[];
			status: QuestionStatus;
	  }
	| { kind: 'notice'; id: string; code: 'not_restored'; text: string };

export type QuestionStatus = 'pending' | 'answered' | 'cancelled';

/** One question of ez's `_ez/ask_user_question`; plain text only. */
export interface QuestionView {
	question: string;
	header: string;
	multiSelect: boolean;
	options: { label: string; description: string }[];
}

/** The answer to one question, sent back as is (the sandbox checks it against the labels). */
export interface QuestionAnswer {
	question: string;
	selected: string[];
	other: string | null;
}

export interface ChatState {
	items: ChatItem[];
	nextId: number;
}

export const initialChat: ChatState = { items: [], nextId: 1 };

/**
 * A mode id from the agent: ez's modes (`.ez/modes.toml`, ADR-0028) or, on sandboxes still running
 * claude-agent-acp, Claude's `bypassPermissions` / `default` / `plan`. The sandbox decides which
 * ids `session/set_mode` accepts.
 */
export type ModeId = string;
/** Longest mode id the browser accepts (the sandbox's limit). */
export const MAX_MODE_ID_LEN = 64;
/** Claude's bypass mode: switching to it asks for confirmation. */
export const BYPASS_MODE = 'bypassPermissions';

export interface ModeOption {
	id: ModeId;
	name: string;
	description?: string;
}

/** Claude's modes, offered when the agent listed none (sandboxes before ez). */
export const CLAUDE_MODES: readonly ModeOption[] = [
	{ id: BYPASS_MODE, name: 'Bypass' },
	{ id: 'default', name: 'Default' },
	{ id: 'plan', name: 'Plan' }
];

/** The chat mode; `null` elsewhere means unknown (old sandbox or fresh connection). */
export interface ModeState {
	modeId: ModeId;
	/** Advisory only: the sandbox enforces it. */
	bypassAllowed: boolean;
	/** Modes the agent advertised; `null` when the response had none. */
	available: ModeOption[] | null;
}
