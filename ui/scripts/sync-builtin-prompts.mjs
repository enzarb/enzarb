// Copies the engine's built-in agent prompt templates (workflows/crates/engine/prompts) into
// src/lib/workflow/builtin-prompts.json, which the workflow editor shows read-only. A test
// (src/lib/workflow/builtin-prompts.spec.ts) fails when the copy is stale: rerun
// `node scripts/sync-builtin-prompts.mjs` from ui/.
import { readFileSync, writeFileSync } from 'node:fs';

const NAMES = [
	'triage',
	'plan',
	'implement',
	'review',
	'address_comments',
	'resolve_conflicts',
	'task',
	'plan_epic'
];
const dir = new URL('../../workflows/crates/engine/prompts/', import.meta.url);
const out = Object.fromEntries(
	NAMES.map((n) => [n, readFileSync(new URL(`${n}.md`, dir), 'utf8')])
);
writeFileSync(
	new URL('../src/lib/workflow/builtin-prompts.json', import.meta.url),
	JSON.stringify(out, null, '\t') + '\n'
);
