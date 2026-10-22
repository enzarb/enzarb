import { describe, expect, it } from 'vitest';
import {
	addNotice,
	addPermission,
	addUserMessage,
	applyModeUpdate,
	applyPermissionDenied,
	applyUpdate,
	cancelPending,
	offeredModes,
	addQuestion,
	modeName,
	parseModeNotification,
	parseModeUpdate,
	parseQuestions,
	parseSessionModes,
	resolvePermission,
	resolveQuestion
} from './reducer.ts';
import { initialChat, type ChatState, type ModeState, type ToolView } from './types.ts';

describe('mode events', () => {
	const ok = { sessionId: 's', modeId: 'plan', bypassAllowed: true };

	it('accepts a valid _enzarb/mode, including ez mode ids', () => {
		expect(parseModeNotification(ok)).toEqual(ok);
		expect(parseModeNotification({ ...ok, modeId: 'build' })).toEqual({ ...ok, modeId: 'build' });
	});

	it.each([
		['empty modeId', { ...ok, modeId: '' }],
		['oversized modeId', { ...ok, modeId: 'x'.repeat(65) }],
		['control characters', { ...ok, modeId: 'a\nb' }],
		['non-string modeId', { ...ok, modeId: 3 }],
		['non-boolean bypassAllowed', { ...ok, bypassAllowed: 'yes' }],
		['missing bypassAllowed', { sessionId: 's', modeId: 'plan' }],
		['non-string sessionId', { ...ok, sessionId: 1 }],
		['null params', null],
		['string params', 'plan']
	])('drops %s', (_n, params) => {
		expect(parseModeNotification(params)).toBeNull();
	});

	it('parses current_mode_update and validates the id', () => {
		expect(
			parseModeUpdate({ sessionUpdate: 'current_mode_update', currentModeId: 'verify' }, 's')
		).toEqual({ sessionId: 's', modeId: 'verify' });
		expect(
			parseModeUpdate({ sessionUpdate: 'current_mode_update', currentModeId: '' }, 's')
		).toBeNull();
		expect(parseModeUpdate({ sessionUpdate: 'plan' }, 's')).toBeNull();
	});

	it('a current_mode_update keeps bypassAllowed and available, or uses false when unseen', () => {
		const available = [{ id: 'default', name: 'Default' }];
		const seen: ModeState = { modeId: 'default', bypassAllowed: true, available };
		expect(applyModeUpdate(seen, { sessionId: 's', modeId: 'plan' })).toEqual({
			modeId: 'plan',
			bypassAllowed: true,
			available
		});
		expect(applyModeUpdate(null, { sessionId: 's', modeId: 'plan' })).toEqual({
			modeId: 'plan',
			bypassAllowed: false,
			available: null
		});
		expect(applyModeUpdate(seen, ok as never).bypassAllowed).toBe(true);
	});

	it('applyUpdate ignores current_mode_update', () => {
		expect(
			applyUpdate(initialChat, { sessionUpdate: 'current_mode_update', currentModeId: 'plan' })
		).toBe(initialChat);
	});
});

describe('parseSessionModes', () => {
	it('reads the current and advertised modes with names; bypassAllowed starts false', () => {
		expect(
			parseSessionModes({
				currentModeId: 'plan',
				availableModes: [
					{ id: 'plan', name: 'Plan', description: 'Read-only planning' },
					{ id: 'build' }
				]
			})
		).toEqual({
			modeId: 'plan',
			bypassAllowed: false,
			available: [
				{ id: 'plan', name: 'Plan', description: 'Read-only planning' },
				{ id: 'build', name: 'build' }
			]
		});
	});

	it('is null for an invalid currentModeId or non-object', () => {
		expect(parseSessionModes({ currentModeId: '', availableModes: [] })).toBeNull();
		expect(parseSessionModes({ availableModes: [] })).toBeNull();
		expect(parseSessionModes('plan')).toBeNull();
		expect(parseSessionModes(undefined)).toBeNull();
	});

	it('drops malformed entries and duplicates, and clips names', () => {
		const m = parseSessionModes({
			currentModeId: 'default',
			availableModes: [
				{ id: 'default', name: 'N'.repeat(100) },
				'plan',
				null,
				{ id: 7 },
				{ id: '' },
				{ id: 'default' }
			]
		});
		expect(m?.available).toEqual([{ id: 'default', name: 'N'.repeat(64) }]);
	});

	it('caps the list and keeps available null when absent', () => {
		expect(parseSessionModes({ currentModeId: 'plan' })?.available).toBeNull();
		const many = Array.from({ length: 100 }, (_, i) => ({ id: `m${i}` }));
		expect(
			parseSessionModes({ currentModeId: 'plan', availableModes: many })?.available
		).toHaveLength(64);
	});
});

describe('offeredModes', () => {
	const m = (over: Partial<ModeState>): ModeState => ({
		modeId: 'default',
		bypassAllowed: false,
		available: null,
		...over
	});
	const ids = (s: ModeState) => offeredModes(s).map((o) => o.id);
	const opts = (...list: string[]) => list.map((id) => ({ id, name: id }));

	it('offers exactly the advertised modes (ez)', () => {
		expect(ids(m({ available: opts('plan', 'build', 'verify') }))).toEqual([
			'plan',
			'build',
			'verify'
		]);
	});

	it('gates bypass on bypassAllowed', () => {
		const all = opts('bypassPermissions', 'default', 'plan');
		expect(ids(m({ available: all }))).toEqual(['default', 'plan']);
		expect(ids(m({ available: all, bypassAllowed: true }))).toEqual([
			'bypassPermissions',
			'default',
			'plan'
		]);
	});

	it("falls back to Claude's modes minus bypass when nothing is advertised", () => {
		expect(ids(m({}))).toEqual(['default', 'plan']);
		expect(ids(m({ bypassAllowed: true }))).toEqual(['bypassPermissions', 'default', 'plan']);
	});

	it('does not offer bypass even when it is the active mode and not allowed', () => {
		expect(ids(m({ modeId: 'bypassPermissions' }))).not.toContain('bypassPermissions');
	});
});

describe('modeName', () => {
	it("uses the agent's name, then Claude's, then the id", () => {
		const mode: ModeState = {
			modeId: 'build',
			bypassAllowed: false,
			available: [{ id: 'build', name: 'Build it' }]
		};
		expect(modeName(mode, 'build')).toBe('Build it');
		expect(modeName(null, 'plan')).toBe('Plan');
		expect(modeName(mode, 'other')).toBe('other');
	});
});

describe('questions', () => {
	const q = {
		question: 'Which DB?',
		header: 'DB',
		multiSelect: false,
		options: [
			{ label: 'sqlite', description: 'file' },
			{ label: 'postgres', description: '' }
		]
	};

	it('parses an _ez/ask_user_question', () => {
		expect(parseQuestions({ sessionId: 's', questions: [q] })).toEqual([q]);
		expect(parseQuestions({ questions: [{ question: 'Q', options: [{ label: 'a' }] }] })).toEqual([
			{ question: 'Q', header: '', multiSelect: false, options: [{ label: 'a', description: '' }] }
		]);
	});

	it.each([
		['no questions', { questions: [] }],
		['not a list', { questions: 'x' }],
		['empty question', { questions: [{ ...q, question: '' }] }],
		['empty label', { questions: [{ ...q, options: [{ label: '' }] }] }],
		['duplicate labels', { questions: [{ ...q, options: [{ label: 'a' }, { label: 'a' }] }] }],
		['too many questions', { questions: Array.from({ length: 9 }, () => q) }],
		['null', null]
	])('rejects %s', (_n, params) => {
		expect(parseQuestions(params)).toBeNull();
	});

	it('adds, resolves and cancels question items', () => {
		let s = addQuestion(initialChat, 'question-1', [q]);
		expect(s.items[0]).toMatchObject({ kind: 'question', status: 'pending' });
		expect(cancelPending(s).items[0]).toMatchObject({ status: 'cancelled' });
		s = resolveQuestion(s, 'question-1', 'answered');
		expect(s.items[0]).toMatchObject({ status: 'answered' });
		// only pending items change
		expect(resolveQuestion(s, 'question-1', 'cancelled')).toBe(s);
	});
});

const text = (t: string) => ({ type: 'text', text: t });
const run = (updates: unknown[], from: ChatState = initialChat) =>
	updates.reduce<ChatState>((s, u) => applyUpdate(s, u), from);
const tool: ToolView = { toolCallId: 't1', title: 'Run ls', text: '' };

describe('applyUpdate: text chunks', () => {
	it.each([
		[
			'agent chunks merge',
			[
				['agent_message_chunk', 'He'],
				['agent_message_chunk', 'llo']
			],
			[['agent', 'Hello']]
		],
		[
			'user chunks merge',
			[
				['user_message_chunk', 'a'],
				['user_message_chunk', 'b']
			],
			[['user', 'ab']]
		],
		[
			'interleaved roles stay separate',
			[
				['user_message_chunk', 'q'],
				['agent_message_chunk', 'a'],
				['agent_thought_chunk', 't'],
				['agent_message_chunk', 'b']
			],
			[
				['user', 'q'],
				['agent', 'a'],
				['thought', 't'],
				['agent', 'b']
			]
		]
	])('%s', (_name, chunks, want) => {
		const s = run(chunks.map(([k, t]) => ({ sessionUpdate: k, content: text(t) })));
		expect(s.items.map((i) => [i.kind, (i as { text: string }).text])).toEqual(want);
	});

	it('does not merge chunks with different message ids', () => {
		const s = run([
			{ sessionUpdate: 'agent_message_chunk', messageId: 'a', content: text('1') },
			{ sessionUpdate: 'agent_message_chunk', messageId: 'b', content: text('2') }
		]);
		expect(s.items).toHaveLength(2);
	});

	it('shows non-text blocks as placeholders', () => {
		const s = run([
			{ sessionUpdate: 'agent_message_chunk', content: { type: 'image', data: 'AAAA' } }
		]);
		expect(s.items[0]).toMatchObject({ text: '[image]' });
	});

	it('is immutable', () => {
		const before = run([{ sessionUpdate: 'agent_message_chunk', content: text('a') }]);
		const snapshot = JSON.stringify(before);
		applyUpdate(before, { sessionUpdate: 'agent_message_chunk', content: text('b') });
		expect(JSON.stringify(before)).toBe(snapshot);
	});

	it.each([
		[null],
		['x'],
		[{}],
		[{ sessionUpdate: 'available_commands_update' }],
		[{ sessionUpdate: 'nope' }]
	])('ignores unknown update %j', (u) => {
		expect(applyUpdate(initialChat, u)).toBe(initialChat);
	});
});

describe('applyUpdate: tool calls', () => {
	it('merges an update into its call by id', () => {
		const s = run([
			{
				sessionUpdate: 'tool_call',
				toolCallId: 't1',
				title: 'Run ls',
				kind: 'execute',
				status: 'pending'
			},
			{
				sessionUpdate: 'tool_call_update',
				toolCallId: 't1',
				status: 'completed',
				content: [{ type: 'content', content: text('out') }]
			}
		]);
		expect(s.items).toHaveLength(1);
		expect(s.items[0]).toMatchObject({
			kind: 'tool',
			title: 'Run ls',
			toolKind: 'execute',
			status: 'completed',
			text: 'out'
		});
	});

	it('creates a stub when the update arrives first, then fills in the call', () => {
		const s = run([
			{ sessionUpdate: 'tool_call_update', toolCallId: 't1', status: 'in_progress' },
			{ sessionUpdate: 'tool_call', toolCallId: 't1', title: 'Late title' }
		]);
		expect(s.items).toHaveLength(1);
		expect(s.items[0]).toMatchObject({ title: 'Late title', status: 'in_progress' });
	});

	it('keeps separate calls separate and ignores ones without an id', () => {
		const s = run([
			{ sessionUpdate: 'tool_call', toolCallId: 'a', title: 'A' },
			{ sessionUpdate: 'tool_call', toolCallId: 'b', title: 'B' },
			{ sessionUpdate: 'tool_call', title: 'no id' }
		]);
		expect(s.items.map((i) => (i as ToolView).title)).toEqual(['A', 'B']);
	});

	it('renders diffs as text', () => {
		const s = run([
			{
				sessionUpdate: 'tool_call',
				toolCallId: 'a',
				content: [{ type: 'diff', path: '/x.ts', newText: 'z' }]
			}
		]);
		expect(s.items[0]).toMatchObject({ text: 'Edit /x.ts' });
	});
});

describe('applyUpdate: plan', () => {
	it('replaces the previous plan in place', () => {
		const s = run([
			{ sessionUpdate: 'plan', entries: [{ content: 'one', status: 'pending', priority: 'high' }] },
			{ sessionUpdate: 'agent_message_chunk', content: text('x') },
			{
				sessionUpdate: 'plan',
				entries: [{ content: 'two', status: 'completed', priority: 'low' }, { nope: 1 }]
			}
		]);
		const plans = s.items.filter((i) => i.kind === 'plan');
		expect(plans).toHaveLength(1);
		expect(plans[0]).toMatchObject({
			entries: [{ content: 'two', status: 'completed', priority: 'low' }]
		});
		expect(s.items[0].kind).toBe('plan');
	});
});

describe('permission items', () => {
	const options = [
		{ optionId: 'a', name: 'Allow', kind: 'allow_once' as const },
		{ optionId: 'r', name: 'Reject', kind: 'reject_once' as const }
	];
	const pending = () => addPermission(initialChat, { id: 'p1', toolCall: tool, options });

	it('starts pending, resolves once, then is final', () => {
		let s = resolvePermission(pending(), 'p1', 'allowed');
		expect(s.items[0]).toMatchObject({ status: 'allowed' });
		s = resolvePermission(s, 'p1', 'denied');
		expect(s.items[0]).toMatchObject({ status: 'allowed' });
	});

	it('cancelPending cancels only pending items', () => {
		let s = addPermission(pending(), { id: 'p2', toolCall: tool, options });
		s = resolvePermission(s, 'p2', 'denied');
		s = cancelPending(s);
		expect(s.items.map((i) => (i as { status: string }).status)).toEqual(['cancelled', 'denied']);
	});

	it('cancelPending returns the same state when nothing is pending', () => {
		expect(cancelPending(initialChat)).toBe(initialChat);
	});

	it('a hint is text only and does not touch the options', () => {
		const s = addPermission(initialChat, {
			id: 'p',
			toolCall: tool,
			options,
			hint: 'Sandbox policy: would deny — r1'
		});
		expect(s.items[0]).toMatchObject({
			status: 'pending',
			hint: 'Sandbox policy: would deny — r1',
			options
		});
	});
});

describe('_enzarb/permission_denied', () => {
	const good = {
		sessionId: 's1',
		toolCall: { toolCallId: 't9', title: 'rm -rf /' },
		rule: 'no-rm',
		reason: 'destructive'
	};

	it('creates a blocked item with no buttons', () => {
		const s = applyPermissionDenied(initialChat, good, 's1');
		expect(s.items[0]).toMatchObject({
			kind: 'permission',
			status: 'blocked',
			options: [],
			reason: 'destructive'
		});
	});

	it.each([
		['another session', { ...good, sessionId: 's2' }],
		['no session id', { ...good, sessionId: undefined }],
		['missing reason', { ...good, reason: undefined }],
		['non-string rule', { ...good, rule: 4 }],
		['bad toolCall', { ...good, toolCall: 'x' }],
		['not an object', 'x']
	])('ignores %s', (_n, params) => {
		expect(applyPermissionDenied(initialChat, params, 's1')).toBe(initialChat);
	});

	it('ignores everything without a current session', () => {
		expect(applyPermissionDenied(initialChat, good, null)).toBe(initialChat);
	});

	it('cancelPending leaves it blocked', () => {
		const s = cancelPending(applyPermissionDenied(initialChat, good, 's1'));
		expect(s.items[0]).toMatchObject({ status: 'blocked' });
	});
});

describe('local items', () => {
	it('adds a user message and a notice', () => {
		const s = addNotice(addUserMessage(initialChat, 'hi'), 'not_restored', 'n');
		expect(s.items.map((i) => i.kind)).toEqual(['user', 'notice']);
	});
});
