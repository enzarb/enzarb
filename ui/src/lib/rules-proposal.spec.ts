import { describe, expect, it, vi } from 'vitest';
import { buildProposal, type ProposalDeps } from '#lib/rules-proposal.ts';
import { exampleWorkflow, toWire } from '#lib/workflows-types.ts';

const saved = toWire({ workflows: [exampleWorkflow('fix-issue')] });
const base = { file: saved, modes: { defaultMode: '', modes: [] }, modesToml: null };

function deps(files: Record<string, string>, over: Partial<ProposalDeps> = {}): ProposalDeps {
	return {
		readFile: vi.fn(async (path: string) =>
			path in files
				? { ok: true as const, path, content: files[path] }
				: { ok: false as const, path, code: 'not_found', message: 'missing' }
		),
		parseModes: vi.fn(async () => ({
			ok: true as const,
			doc: {
				defaultMode: 'a',
				modes: [
					{
						id: 'a',
						description: '',
						prompt: '',
						tools: null,
						miseTasks: [],
						mcpTools: [],
						next: [],
						autoAdvanceTo: [],
						permissions: {},
						hooks: {}
					}
				]
			},
			issues: []
		})),
		checkRules: vi.fn(async () => ({
			ok: true as const,
			diagnostics: [],
			file: { ...saved, workflows: { ...saved.workflows, extra: saved.workflows!['fix-issue'] } }
		})),
		...over
	};
}

describe('buildProposal', () => {
	it('reads both files under the base and diffs them', async () => {
		const d = deps({ '/w/.ez/workflows.toml': 'w', '/w/.ez/modes.toml': 'm' });
		const r = await buildProposal('/w/', d, base);
		expect(d.readFile).toHaveBeenCalledWith('/w/.ez/workflows.toml');
		expect(d.readFile).toHaveBeenCalledWith('/w/.ez/modes.toml');
		expect(r.ok && r.proposal?.diff.addedRules).toEqual(['extra']);
		expect(r.ok && r.proposal?.diff.addedModes).toEqual(['a']);
		expect(r.ok && r.proposal?.rulesChanged).toBe(true);
		expect(r.ok && r.proposal?.modesChanged).toBe(true);
		expect(d.checkRules).toHaveBeenCalledWith('w', 'm');
	});

	it('returns no proposal when nothing differs', async () => {
		const d = deps(
			{ '.ez/workflows.toml': 'w' },
			{ checkRules: vi.fn(async () => ({ ok: true as const, diagnostics: [], file: saved })) }
		);
		expect(await buildProposal('', d, base)).toEqual({ ok: true, proposal: null });
	});

	it('reports validation errors instead of a proposal', async () => {
		const d = deps(
			{ '.ez/workflows.toml': 'w' },
			{
				checkRules: vi.fn(async () => ({
					ok: true as const,
					diagnostics: [{ severity: 'error' as const, path: 'x', message: 'unknown mode' }],
					file: null
				}))
			}
		);
		expect(await buildProposal('', d, base)).toEqual({
			ok: false,
			message: '.ez/workflows.toml has an error: unknown mode'
		});
	});

	it('surfaces a refused read (path escape) as an error', async () => {
		const d = deps(
			{},
			{
				readFile: vi.fn(async (path: string) => ({
					ok: false as const,
					path,
					code: 'outside_workspace',
					message: 'cannot read that path'
				}))
			}
		);
		expect((await buildProposal('/etc/', d, base)).ok).toBe(false);
	});
});
