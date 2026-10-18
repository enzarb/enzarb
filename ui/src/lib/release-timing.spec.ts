import { describe, expect, it } from 'vitest';
import { imageTag, releaseTimings, type TimedRelease } from './release-timing.ts';

const rel = (version: string, publishedAt: string | null, extra: Partial<TimedRelease> = {}) => ({
	version,
	url: `https://github.com/enzarb/enzarb/releases/tag/helm/${version}`,
	publishedAt,
	...extra
});
const dep = (version: string, deployedAt: string) => ({ version, deployedAt });
const roll = (at: string, image?: string) => ({ at, reason: 'image/sandbox: a -> b', image });

describe('imageTag', () => {
	it('reads the tag and ignores registry ports, digests and a leading v', () => {
		expect(imageTag('ghcr.io/enzarb/enzarb-sandbox:0.34.0')).toBe('0.34.0');
		expect(imageTag('ghcr.io/enzarb/enzarb-sandbox:v0.34.0@sha256:abc')).toBe('0.34.0');
		expect(imageTag('localhost:5000/sandbox')).toBeNull();
		expect(imageTag('sandbox')).toBeNull();
		expect(imageTag(undefined)).toBeNull();
	});
});

describe('releaseTimings', () => {
	it('joins the full case by image version', () => {
		const t = releaseTimings(
			[
				rel('v0.10.0', '2026-10-02T10:00:00Z', { sandboxImageVersion: 'v0.35.0' }),
				rel('v0.9.0', '2026-10-01T10:00:00Z', { sandboxImageVersion: '0.34.0' })
			],
			[dep('0.10.0', '2026-10-02T10:04:00Z'), dep('0.9.0', '2026-10-01T10:03:00Z')],
			[
				roll('2026-10-02T11:00:00Z', 'ghcr.io/enzarb/enzarb-sandbox:0.35.0'),
				roll('2026-10-02T10:30:00Z', 'ghcr.io/enzarb/enzarb-sandbox:0.35.0'),
				roll('2026-10-01T10:20:00Z', 'ghcr.io/enzarb/enzarb-sandbox:0.34.0')
			]
		);
		expect(t[0]).toMatchObject({
			version: 'v0.10.0',
			deployedAt: '2026-10-02T10:04:00.000Z',
			sandboxAppliedAt: '2026-10-02T10:30:00.000Z',
			mergeToDeployedSeconds: 240,
			deployToSandboxSeconds: 26 * 60,
			skew: false
		});
		expect(t[1].sandboxAppliedAt).toBe('2026-10-01T10:20:00.000Z');
		expect(t[1].mergeToDeployedSeconds).toBe(180);
	});

	it('falls back to the time window for a roll without a usable image', () => {
		const t = releaseTimings(
			[rel('v0.10.0', '2026-10-02T10:00:00Z'), rel('v0.9.0', '2026-10-01T10:00:00Z')],
			[dep('0.10.0', '2026-10-02T10:04:00Z'), dep('0.9.0', '2026-10-01T10:03:00Z')],
			[roll('2026-10-01T12:00:00Z'), roll('2026-10-02T12:00:00Z', 'sandbox')]
		);
		expect(t[1].sandboxAppliedAt).toBe('2026-10-01T12:00:00.000Z');
		expect(t[0].sandboxAppliedAt).toBe('2026-10-02T12:00:00.000Z');
	});

	it('does not hand a roll claimed by another release to the window', () => {
		const t = releaseTimings(
			[
				rel('v0.10.0', '2026-10-02T10:00:00Z'),
				rel('v0.9.0', '2026-10-01T10:00:00Z', { sandboxImageVersion: '0.34.0' })
			],
			[dep('0.10.0', '2026-10-02T10:04:00Z'), dep('0.9.0', '2026-10-01T10:03:00Z')],
			[roll('2026-10-02T12:00:00Z', 'img:0.34.0')]
		);
		expect(t[0].sandboxAppliedAt).toBeNull();
		expect(t[1].sandboxAppliedAt).toBe('2026-10-02T12:00:00.000Z');
	});

	it('shows the stages it has when there is no sandbox change', () => {
		const [t] = releaseTimings(
			[rel('v0.9.0', '2026-10-01T10:00:00Z')],
			[dep('0.9.0', '2026-10-01T10:03:00Z')],
			[]
		);
		expect(t).toMatchObject({
			mergeToDeployedSeconds: 180,
			sandboxAppliedAt: null,
			deployToSandboxSeconds: null
		});
	});

	it('shows the merge time only when the deploy was not recorded', () => {
		const [t] = releaseTimings(
			[rel('v0.8.0', '2026-09-01T10:00:00Z')],
			[dep('0.9.0', '2026-10-01T10:03:00Z')],
			[roll('2026-10-01T12:00:00Z')]
		);
		expect(t).toMatchObject({
			publishedAt: '2026-09-01T10:00:00Z',
			deployedAt: null,
			sandboxAppliedAt: null,
			mergeToDeployedSeconds: null,
			skew: false
		});
	});

	it('flags negative skew instead of a negative duration', () => {
		const [t] = releaseTimings(
			[rel('v0.9.0', '2026-10-01T10:05:00Z')],
			[dep('0.9.0', '2026-10-01T10:03:00Z')],
			[]
		);
		expect(t).toMatchObject({ mergeToDeployedSeconds: null, skew: true });
		expect(t.deployedAt).not.toBeNull();
	});

	it('has no merge-to-deployed time without publishedAt', () => {
		const [t] = releaseTimings([rel('v0.9.0', null)], [dep('0.9.0', '2026-10-01T10:03:00Z')], []);
		expect(t).toMatchObject({ publishedAt: null, mergeToDeployedSeconds: null, skew: false });
		expect(t.deployedAt).not.toBeNull();
	});

	it('limits to the newest releases and ignores bad roll times', () => {
		const rs = Array.from({ length: 8 }, (_, i) => rel(`v0.${8 - i}.0`, null));
		expect(releaseTimings(rs, [], [roll('nope')])).toHaveLength(5);
		expect(releaseTimings(rs, [], [], 2)).toHaveLength(2);
	});
});
