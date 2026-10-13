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
	parseModeNotification,
	parseModeUpdate,
	parseSessionModes,
	resolvePermission
} from './reducer.ts';
import { initialChat, type ChatState, type ModeState, type ToolView } from './types.ts';

describe('mode events', () => {
	const ok = { sessionId: 's', modeId: 'plan', bypassAllowed: true };

	it('accepts a valid _enzarb/mode', () => {
		expect(parseModeNotification(ok)).toEqual(ok);
	});

	it.each([
		['unknown modeId', { ...ok, modeId: 'acceptEdits' }],
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
			parseModeUpdate({ sessionUpdate: 'current_mode_update', currentModeId: 'default' }, 's')
		).toEqual({ sessionId: 's', modeId: 'default' });
		expect(
			parseModeUpdate({ sessionUpdate: 'current_mode_update', currentModeId: 'x' }, 's')
		).toBeNull();
		expect(parseModeUpdate({ sessionUpdate: 'plan' }, 's')).toBeNull();
	});

	it('a current_mode_update keeps bypassAllowed and available, or uses false when unseen', () => {
		const seen: ModeState = { modeId: 'default', bypassAllowed: true, available: ['default'] };
		expect(applyModeUpdate(seen, { sessionId: 's', modeId: 'plan' })).toEqual({
			modeId: 'plan',
			bypassAllowed: true,
			available: ['default']
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
	it('reads the current and advertised modes; bypassAllowed starts false', () => {
		expect(
			parseSessionModes({
				currentModeId: 'bypassPermissions',
				availableModes: [{ id: 'default' }, { id: 'plan' }]
			})
		).toEqual({
			modeId: 'bypassPermissions',
			bypassAllowed: false,
			available: ['default', 'plan']
		});
	});

	it('is null for an invalid currentModeId or non-object', () => {
		expect(parseSessionModes({ currentModeId: 'nope', availableModes: [] })).toBeNull();
		expect(parseSessionModes({ availableModes: [] })).toBeNull();
		expect(parseSessionModes('plan')).toBeNull();
		expect(parseSessionModes(undefined)).toBeNull();
	});

	it('drops unknown and malformed entries and duplicates', () => {
		const m = parseSessionModes({
			currentModeId: 'default',
			availableModes: [
				{ id: 'default' },
				{ id: 'evil' },
				'plan',
				null,
				{ id: 7 },
				{ id: 'default' }
			]
		});
		expect(m?.available).toEqual(['default']);
	});

	it('caps the list and keeps available null when absent', () => {
		expect(parseSessionModes({ currentModeId: 'plan' })?.available).toBeNull();
		const many = Array.from({ length: 100 }, () => ({ id: 'x' }));
		expect(
			parseSessionModes({ currentModeId: 'plan', availableModes: [...many, { id: 'default' }] })
				?.available
		).toEqual([]);
	});
});

describe('offeredModes', () => {
	const m = (over: Partial<ModeState>): ModeState => ({
		modeId: 'default',
		bypassAllowed: false,
		available: null,
		...over
	});

	it('intersects with the advertised modes', () => {
		expect(offeredModes(m({ available: ['plan'] }))).toEqual(['plan']);
	});

	it('gates bypass on bypassAllowed', () => {
		const all = ['bypassPermissions', 'default', 'plan'] as const;
		expect(offeredModes(m({ available: [...all] }))).toEqual(['default', 'plan']);
		expect(offeredModes(m({ available: [...all], bypassAllowed: true }))).toEqual([...all]);
	});

	it('falls back to the allow-list minus bypass when nothing is advertised', () => {
		expect(offeredModes(m({}))).toEqual(['default', 'plan']);
		expect(offeredModes(m({ bypassAllowed: true }))).toEqual([
			'bypassPermissions',
			'default',
			'plan'
		]);
	});

	it('does not offer bypass even when it is the active mode and not allowed', () => {
		expect(offeredModes(m({ modeId: 'bypassPermissions' }))).not.toContain('bypassPermissions');
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
