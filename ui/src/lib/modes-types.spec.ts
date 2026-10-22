import { describe, expect, it } from 'vitest';
import {
	blankMode,
	freeModeId,
	issueModeIndex,
	removeMode,
	renameMode,
	type ModesDoc
} from './modes-types.ts';

const doc = (): ModesDoc => ({
	defaultMode: 'plan',
	modes: [
		{ ...blankMode('plan'), originalId: 'plan', next: ['build'] },
		{ ...blankMode('build'), originalId: 'build', next: ['plan'], autoAdvanceTo: ['plan'] }
	]
});

describe('modes document helpers', () => {
	it('renames a mode and its references, keeping originalId', () => {
		const d = renameMode(doc(), 0, 'scout');
		expect(d.defaultMode).toBe('scout');
		expect(d.modes[0]).toMatchObject({ id: 'scout', originalId: 'plan' });
		expect(d.modes[1].next).toEqual(['scout']);
		expect(d.modes[1].autoAdvanceTo).toEqual(['scout']);
	});

	it('removes a mode, its references and moves the default', () => {
		const d = removeMode(doc(), 0);
		expect(d.defaultMode).toBe('build');
		expect(d.modes.map((m) => m.id)).toEqual(['build']);
		expect(d.modes[0].next).toEqual([]);
	});

	it('finds free ids and the mode an issue points into', () => {
		expect(freeModeId(doc(), 'plan')).toBe('plan-2');
		expect(freeModeId(doc(), 'verify')).toBe('verify');
		expect(issueModeIndex('modes[1].tools[0]')).toBe(1);
		expect(issueModeIndex('defaultMode')).toBeNull();
	});
});
