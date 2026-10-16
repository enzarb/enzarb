import { describe, expect, it } from 'vitest';
import { jobStatus, stepStatus } from './components/ui/status.ts';
import { applyTheme, readTheme, writeTheme, THEME_KEY } from './theme.ts';

describe('status tones', () => {
	it('maps run results and project steps', () => {
		expect(jobStatus('succeeded').tone).toBe('success');
		expect(jobStatus('failed').tone).toBe('danger');
		expect(jobStatus('running').tone).toBe('info');
		expect(jobStatus('queued').hollow).toBe(true);
		expect(stepStatus.done.tone).toBe('success');
		expect(stepStatus.failed.tone).toBe('danger');
	});
});

describe('theme storage', () => {
	const memory = (): Storage => {
		const m = new Map<string, string>();
		return {
			getItem: (k) => m.get(k) ?? null,
			setItem: (k, v) => void m.set(k, v),
			removeItem: (k) => void m.delete(k)
		} as Storage;
	};
	const broken = {
		getItem: () => {
			throw new Error('blocked');
		},
		setItem: () => {
			throw new Error('blocked');
		},
		removeItem: () => {
			throw new Error('blocked');
		}
	} as unknown as Storage;

	it('round-trips light and dark, and forgets system', () => {
		const s = memory();
		expect(readTheme(s)).toBe('system');
		writeTheme('dark', s);
		expect(s.getItem(THEME_KEY)).toBe('dark');
		expect(readTheme(s)).toBe('dark');
		writeTheme('system', s);
		expect(readTheme(s)).toBe('system');
	});

	it('ignores junk and survives blocked storage', () => {
		const s = memory();
		s.setItem(THEME_KEY, 'neon');
		expect(readTheme(s)).toBe('system');
		expect(readTheme(broken)).toBe('system');
		expect(() => writeTheme('dark', broken)).not.toThrow();
		expect(readTheme(undefined)).toBe('system');
	});

	it('applies data-theme only for forced themes', () => {
		const root = {
			attrs: new Map<string, string>(),
			setAttribute(k: string, v: string) {
				this.attrs.set(k, v);
			},
			removeAttribute(k: string) {
				this.attrs.delete(k);
			}
		};
		applyTheme('dark', root as unknown as HTMLElement);
		expect(root.attrs.get('data-theme')).toBe('dark');
		applyTheme('system', root as unknown as HTMLElement);
		expect(root.attrs.has('data-theme')).toBe(false);
	});
});
