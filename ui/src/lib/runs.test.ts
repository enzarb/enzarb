import { describe, expect, it } from 'vitest';
import { branchUrl, isEnded, nodeText, runTone, usageText } from './runs.ts';

describe('runs helpers', () => {
	it('knows ended runs and tones', () => {
		expect(isEnded({ status: 'cancelled' })).toBe(true);
		expect(isEnded({ status: 'running' })).toBe(false);
		expect(runTone('failed').tone).toBe('danger');
		expect(runTone('pending').hollow).toBe(true);
	});

	it('links only pushed branches, encoding the path', () => {
		expect(branchUrl({ repo: 'o/r', branch: 'ez/a b/x', head_sha: '', pushed: true })).toBe(
			'https://github.com/o/r/tree/ez/a%20b/x'
		);
		expect(branchUrl({ repo: 'o/r', branch: 'b', head_sha: '', pushed: false })).toBeNull();
	});

	it('shows the instruction or the command', () => {
		expect(nodeText({ type: 'agent', instruction: 'hi' })).toBe('hi');
		expect(nodeText({ type: 'command', task: 'build', args: ['a', 'b'] })).toBe(
			'mise run build -- a b'
		);
		expect(nodeText({ type: 'command', script: 'make' })).toBe('make');
		expect(nodeText(null)).toBe('');
		expect(usageText({ input_tokens: 1, output_tokens: 2 })).toBe('1 in, 2 out');
		expect(usageText(null)).toBe('');
	});
});
