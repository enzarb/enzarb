import { NAMED_PROMPTS } from './definition.ts';

// Read straight from the engine's templates at build time, so the UI can never drift from them.
const files = import.meta.glob<string>('../../../../workflows/crates/engine/prompts/*.md', {
	query: '?raw',
	import: 'default',
	eager: true
});

const byName = Object.fromEntries(
	Object.entries(files).map(([path, text]) => [path.slice(path.lastIndexOf('/') + 1, -3), text])
);

/** The engine's built-in agent prompt templates, by name. */
export const BUILTIN_PROMPTS = Object.fromEntries(
	NAMED_PROMPTS.map((name) => [name, byName[name] ?? ''])
) as Record<(typeof NAMED_PROMPTS)[number], string>;
