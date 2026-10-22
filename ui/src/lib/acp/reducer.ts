import {
	BYPASS_MODE,
	CLAUDE_MODES,
	MAX_MODE_ID_LEN,
	type ChatItem,
	type ChatState,
	type ModeId,
	type ModeOption,
	type ModeState,
	type QuestionStatus,
	type QuestionView,
	type PermissionOption,
	type PermissionStatus,
	type PlanEntry,
	type TextRole,
	type ToolView
} from './types.ts';

const MAX_TEXT = 2000;

type Obj = Record<string, unknown>;

function obj(v: unknown): Obj | null {
	return typeof v === 'object' && v !== null ? (v as Obj) : null;
}

function str(v: unknown, max = MAX_TEXT): string | undefined {
	return typeof v === 'string' ? v.slice(0, max) : undefined;
}

/** Plain text of a content block; non-text blocks become a short placeholder. */
function blockText(block: unknown): string {
	const b = obj(block);
	if (!b) return '';
	if (b.type === 'text') return typeof b.text === 'string' ? b.text : '';
	if (b.type === 'resource_link') return `[${str(b.name, 200) ?? 'resource'}]`;
	return typeof b.type === 'string' ? `[${b.type.slice(0, 20)}]` : '';
}

function toolContentText(content: unknown): string {
	if (!Array.isArray(content)) return '';
	const parts: string[] = [];
	for (const c of content) {
		const o = obj(c);
		if (!o) continue;
		if (o.type === 'content') parts.push(blockText(o.content));
		else if (o.type === 'diff') parts.push(`Edit ${str(o.path, 500) ?? ''}`.trim());
		else if (o.type === 'terminal') parts.push('[terminal]');
	}
	return parts.filter(Boolean).join('\n');
}

/** Build a ToolView from a `tool_call`, `tool_call_update` or permission `toolCall`; `prev` fills gaps. */
export function toolView(raw: unknown, prev?: ToolView): ToolView | null {
	const o = obj(raw);
	const id = str(o?.toolCallId, 256);
	if (!o || !id) return null;
	const content = o.content !== undefined ? toolContentText(o.content) : undefined;
	return {
		toolCallId: id,
		title: str(o.title, 500) ?? prev?.title ?? '',
		toolKind: str(o.kind, 40) ?? prev?.toolKind,
		status: str(o.status, 40) ?? prev?.status,
		text: content ?? prev?.text ?? ''
	};
}

function push(state: ChatState, make: (id: string) => ChatItem): ChatState {
	const item = make(`i${state.nextId}`);
	return { items: [...state.items, item], nextId: state.nextId + 1 };
}

function appendText(state: ChatState, role: TextRole, text: string, messageId?: string): ChatState {
	const last = state.items.at(-1);
	if (last && last.kind === role && last.messageId === messageId) {
		const items = state.items.slice(0, -1);
		items.push({ ...last, text: last.text + text });
		return { ...state, items };
	}
	return push(state, (id) => ({ kind: role, id, text, messageId }));
}

function replaceAt(state: ChatState, i: number, item: ChatItem): ChatState {
	const items = state.items.slice();
	items[i] = item;
	return { ...state, items };
}

function upsertTool(state: ChatState, raw: unknown): ChatState {
	const incoming = obj(raw);
	const id = str(incoming?.toolCallId, 256);
	if (!id) return state;
	const i = state.items.findIndex((it) => it.kind === 'tool' && it.toolCallId === id);
	if (i >= 0) {
		const prev = state.items[i] as ChatItem & { kind: 'tool' };
		const view = toolView(raw, prev);
		return view ? replaceAt(state, i, { ...view, kind: 'tool', id: prev.id }) : state;
	}
	const view = toolView(raw);
	return view ? push(state, (iid) => ({ ...view, kind: 'tool', id: iid })) : state;
}

function planEntries(raw: unknown): PlanEntry[] {
	if (!Array.isArray(raw)) return [];
	return raw.flatMap((e) => {
		const o = obj(e);
		const content = str(o?.content);
		return o && content
			? [
					{
						content,
						status: str(o.status, 40) ?? 'pending',
						priority: str(o.priority, 40) ?? 'medium'
					}
				]
			: [];
	});
}

/** Apply one `session/update` payload. Pure; unknown update kinds leave the state unchanged. */
export function applyUpdate(state: ChatState, update: unknown): ChatState {
	const u = obj(update);
	if (!u) return state;
	const chunk = (role: TextRole) =>
		appendText(state, role, blockText(u.content), str(u.messageId, 256));
	switch (u.sessionUpdate) {
		case 'user_message_chunk':
			return chunk('user');
		case 'agent_message_chunk':
			return chunk('agent');
		case 'agent_thought_chunk':
			return chunk('thought');
		case 'tool_call':
		case 'tool_call_update':
			return upsertTool(state, u);
		case 'plan': {
			const entries = planEntries(u.entries);
			const i = state.items.findIndex((it) => it.kind === 'plan');
			if (i >= 0) return replaceAt(state, i, { kind: 'plan', id: state.items[i].id, entries });
			return push(state, (id) => ({ kind: 'plan', id, entries }));
		}
		default:
			return state;
	}
}

/** The local user's prompt, shown at once (the agent does not echo it). */
export function addUserMessage(state: ChatState, text: string): ChatState {
	return push(state, (id) => ({ kind: 'user', id, text }));
}

export function addNotice(state: ChatState, code: 'not_restored', text: string): ChatState {
	return push(state, (id) => ({ kind: 'notice', id, code, text }));
}

export interface PermissionInput {
	id: string;
	toolCall: ToolView;
	options: PermissionOption[];
	status?: PermissionStatus;
	hint?: string;
	reason?: string;
}

export function addPermission(state: ChatState, p: PermissionInput): ChatState {
	return push(state, () => ({
		kind: 'permission',
		id: p.id,
		toolCall: p.toolCall,
		options: p.options,
		status: p.status ?? 'pending',
		hint: p.hint,
		reason: p.reason
	}));
}

/** Move a still-pending permission to `status`; anything else is a no-op. */
export function resolvePermission(
	state: ChatState,
	id: string,
	status: Exclude<PermissionStatus, 'pending'>
): ChatState {
	const i = state.items.findIndex((it) => it.kind === 'permission' && it.id === id);
	const it = state.items[i];
	if (!it || it.kind !== 'permission' || it.status !== 'pending') return state;
	return replaceAt(state, i, { ...it, status });
}

/** Every pending permission becomes `cancelled` (close, reconnect, Stop, unmount). */
export function cancelPending(state: ChatState): ChatState {
	const pending = (it: ChatItem) =>
		(it.kind === 'permission' || it.kind === 'question') && it.status === 'pending';
	if (!state.items.some(pending)) return state;
	return {
		...state,
		items: state.items.map((it) =>
			pending(it) ? ({ ...it, status: 'cancelled' } as ChatItem) : it
		)
	};
}

/**
 * The sandbox's `_enzarb/permission_denied` notification: `{sessionId, toolCall, rule, reason}`.
 * Ignored when malformed or for another session; produces a button-less `blocked` item.
 */
export function applyPermissionDenied(
	state: ChatState,
	params: unknown,
	currentSessionId: string | null
): ChatState {
	const p = obj(params);
	if (!p || !currentSessionId || p.sessionId !== currentSessionId) return state;
	const tool = toolView(p.toolCall);
	const rule = str(p.rule, 200);
	const reason = str(p.reason, 500);
	if (!tool || !rule || !reason) return state;
	return addPermission(state, {
		id: `blocked-${state.nextId}`,
		toolCall: tool,
		options: [],
		status: 'blocked',
		reason
	});
}

const MAX_AVAILABLE_MODES = 64;
const MAX_MODE_NAME = 64;
const MAX_MODE_DESCRIPTION = 200;

/** A mode id: a non-empty string of at most `MAX_MODE_ID_LEN` printable characters. */
function modeId(v: unknown): ModeId | undefined {
	return typeof v === 'string' &&
		v.length > 0 &&
		v.length <= MAX_MODE_ID_LEN &&
		// eslint-disable-next-line no-control-regex
		!/[\u0000-\u001f\u007f]/.test(v)
		? v
		: undefined;
}

function modeOption(raw: unknown): ModeOption | undefined {
	const o = obj(raw);
	const id = modeId(o?.id);
	if (!o || !id) return undefined;
	const name = typeof o.name === 'string' && o.name.trim() ? o.name.trim() : id;
	const opt: ModeOption = { id, name: name.slice(0, MAX_MODE_NAME) };
	if (typeof o.description === 'string' && o.description.trim())
		opt.description = o.description.trim().slice(0, MAX_MODE_DESCRIPTION);
	return opt;
}

/**
 * The `modes` of a `session/new` / `session/load` response. `bypassAllowed` is not part of it, so
 * it starts `false`. Null when `currentModeId` is missing or unknown.
 */
export function parseSessionModes(raw: unknown): ModeState | null {
	const m = obj(raw);
	const current = modeId(m?.currentModeId);
	if (!m || !current) return null;
	let available: ModeOption[] | null = null;
	if (Array.isArray(m.availableModes)) {
		available = [];
		for (const entry of m.availableModes.slice(0, MAX_AVAILABLE_MODES)) {
			const opt = modeOption(entry);
			if (opt && !available.some((a) => a.id === opt.id)) available.push(opt);
		}
	}
	return { modeId: current, bypassAllowed: false, available };
}

export interface ModeEvent {
	sessionId: string;
	modeId: ModeId;
	/** Absent for `current_mode_update`. */
	bypassAllowed?: boolean;
}

/** The `_enzarb/mode` notification; null when malformed. `sessionId` is checked by the caller. */
export function parseModeNotification(params: unknown): ModeEvent | null {
	const p = obj(params);
	if (!p || typeof p.sessionId !== 'string' || typeof p.bypassAllowed !== 'boolean') return null;
	const id = modeId(p.modeId);
	return id ? { sessionId: p.sessionId, modeId: id, bypassAllowed: p.bypassAllowed } : null;
}

/** The `current_mode_update` session update; null when it is not one or the id is unknown. */
export function parseModeUpdate(update: unknown, sessionId: string): ModeEvent | null {
	const u = obj(update);
	if (!u || u.sessionUpdate !== 'current_mode_update') return null;
	const id = modeId(u.currentModeId);
	return id ? { sessionId, modeId: id } : null;
}

/** Fold a mode event into the mode; `bypassAllowed` keeps its last value (false when unseen). */
export function applyModeUpdate(mode: ModeState | null, ev: ModeEvent): ModeState {
	return {
		modeId: ev.modeId,
		bypassAllowed: ev.bypassAllowed ?? mode?.bypassAllowed ?? false,
		available: mode?.available ?? null
	};
}

/** The modes the switch may request. The active mode is shown as selected regardless. */
export function offeredModes(mode: ModeState): ModeOption[] {
	const base = mode.available ?? CLAUDE_MODES;
	return base.filter((m) => m.id !== BYPASS_MODE || mode.bypassAllowed);
}

/** The display name of `id`: the agent's name for it, Claude's, or the id itself. */
export function modeName(mode: ModeState | null, id: ModeId): string {
	return (
		mode?.available?.find((m) => m.id === id)?.name ??
		CLAUDE_MODES.find((m) => m.id === id)?.name ??
		id
	);
}

const MAX_QUESTIONS = 8;
const MAX_QUESTION_OPTIONS = 16;
const MAX_QUESTION_TEXT = 500;
const MAX_LABEL = 200;

function clip(v: unknown, max: number): string {
	return typeof v === 'string' ? v.slice(0, max) : '';
}

/**
 * The questions of an `_ez/ask_user_question` request; null when malformed (the caller answers
 * `cancelled`). Labels are kept exactly: the answer must repeat them.
 */
export function parseQuestions(params: unknown): QuestionView[] | null {
	const p = obj(params);
	if (!p || !Array.isArray(p.questions) || p.questions.length === 0) return null;
	if (p.questions.length > MAX_QUESTIONS) return null;
	const out: QuestionView[] = [];
	for (const raw of p.questions) {
		const q = obj(raw);
		if (!q || typeof q.question !== 'string' || !q.question) return null;
		if (q.question.length > MAX_QUESTION_TEXT || !Array.isArray(q.options)) return null;
		const options: QuestionView['options'] = [];
		for (const o of q.options.slice(0, MAX_QUESTION_OPTIONS)) {
			const r = obj(o);
			if (!r || typeof r.label !== 'string' || !r.label || r.label.length > MAX_LABEL) return null;
			if (options.some((x) => x.label === r.label)) return null;
			options.push({ label: r.label, description: clip(r.description, MAX_QUESTION_TEXT) });
		}
		out.push({
			question: q.question,
			header: clip(q.header, MAX_LABEL),
			multiSelect: q.multiSelect === true,
			options
		});
	}
	return out;
}

export function addQuestion(state: ChatState, id: string, questions: QuestionView[]): ChatState {
	return {
		items: [...state.items, { kind: 'question', id, questions, status: 'pending' }],
		nextId: state.nextId
	};
}

export function resolveQuestion(state: ChatState, id: string, status: QuestionStatus): ChatState {
	const i = state.items.findIndex((it) => it.kind === 'question' && it.id === id);
	const item = state.items[i];
	if (i < 0 || item?.kind !== 'question' || item.status !== 'pending') return state;
	return replaceAt(state, i, { ...item, status });
}
