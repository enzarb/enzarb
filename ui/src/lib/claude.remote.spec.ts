import { isHttpError } from '@sveltejs/kit';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const locals = vi.hoisted(() => ({ user: undefined as unknown }));
const del = vi.hoisted(() => vi.fn());
vi.mock('$app/server', () => ({
	command: (handler: object) => Object.assign(handler, { __: { type: 'command' } }),
	query: (handler: object) => Object.assign(handler, { __: { type: 'query' } }),
	getRequestEvent: () => ({ locals })
}));
vi.mock('#lib/server/db.ts', () => ({ getSql: () => 'sql' }));
vi.mock('#lib/server/claude-credential.ts', () => ({
	getClaudeCredential: vi.fn(),
	deleteOwnClaudeCredential: del
}));

import { removeClaudeCredential } from './claude.remote.ts';

const run = removeClaudeCredential as unknown as (...args: unknown[]) => Promise<void>;

beforeEach(() => {
	del.mockReset();
	del.mockResolvedValue(true);
	locals.user = { id: 1, login: 'octo', name: null };
});

describe('removeClaudeCredential', () => {
	it('is 401 when not signed in and deletes nothing', async () => {
		locals.user = undefined;
		const err = await run().catch((e) => e);
		expect(isHttpError(err, 401)).toBe(true);
		expect(del).not.toHaveBeenCalled();
	});

	it('deletes the session login, ignoring any argument', async () => {
		await run('victim');
		expect(del).toHaveBeenCalledExactlyOnceWith('sql', 'octo');
	});

	it('takes no input', () => {
		expect(removeClaudeCredential.length).toBe(0);
	});
});
