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
	| { kind: 'notice'; id: string; code: 'not_restored'; text: string };

export interface ChatState {
	items: ChatItem[];
	nextId: number;
}

export const initialChat: ChatState = { items: [], nextId: 1 };

/** The permission modes the browser knows; anything else from the agent is dropped. */
export const MODE_IDS = ['bypassPermissions', 'default', 'plan'] as const;
export type ModeId = (typeof MODE_IDS)[number];

/** The chat permission mode; `null` elsewhere means unknown (old sandbox or fresh connection). */
export interface ModeState {
	modeId: ModeId;
	/** Advisory only: the sandbox enforces it. */
	bypassAllowed: boolean;
	/** Modes the agent advertised; `null` when the response had none. */
	available: ModeId[] | null;
}
