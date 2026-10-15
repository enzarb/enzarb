import prompts from './builtin-prompts.json' with { type: 'json' };
import type { NAMED_PROMPTS } from './definition.ts';

/** The engine's built-in agent prompt templates, by name (a checked-in copy; see the spec). */
export const BUILTIN_PROMPTS: Record<(typeof NAMED_PROMPTS)[number], string> = prompts;
