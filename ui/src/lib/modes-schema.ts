import * as v from 'valibot';
import { NameArg } from '#lib/server/owned-project.ts';

/** Input schemas of the modes designer's remote functions (`modes.remote.ts` may export only remote functions). */

export const Str = (max: number) => v.pipe(v.string(), v.maxLength(max));
const List = (max: number, item = 200) => v.pipe(v.array(Str(item)), v.maxLength(max));
const Opt = (max: number) => v.optional(v.nullable(Str(max)));

const HookSchema = v.object({
	run: Str(4000),
	glob: Opt(500),
	matcher: Opt(500),
	onFailure: Str(20),
	maxFixTurns: v.pipe(v.number(), v.integer(), v.minValue(0), v.maxValue(1000)),
	timeoutSecs: v.pipe(v.number(), v.integer(), v.minValue(0), v.maxValue(86_400))
});

const ModeSchema = v.object({
	id: Str(100),
	originalId: v.optional(Str(100)),
	name: Opt(200),
	description: Str(2000),
	prompt: Str(100_000),
	tools: v.nullable(List(200)),
	miseTasks: List(200),
	mcpTools: List(200),
	next: List(100, 100),
	autoAdvanceTo: List(100, 100),
	profile: Opt(200),
	model: Opt(200),
	permissions: v.pipe(
		v.record(Str(200), Str(10)),
		v.check((r) => Object.keys(r).length <= 200)
	),
	hooks: v.pipe(
		v.record(Str(50), v.pipe(v.array(HookSchema), v.maxLength(50))),
		v.check((r) => Object.keys(r).length <= 20)
	)
});

/** The designer's document; tracks does the real validation. */
export const ModesDocSchema = v.object({
	defaultMode: Str(100),
	modes: v.pipe(v.array(ModeSchema), v.maxLength(100))
});

export const RepoArg = v.optional(v.pipe(v.string(), v.maxLength(250)));
export const TEXT_MAX = 256 * 1024;

/** Input of `saveModesFile`: the form's document or the TOML text, never both. */
export const SaveModesSchema = v.pipe(
	v.object({
		name: NameArg,
		repo: v.pipe(v.string(), v.maxLength(250)),
		baseSha: v.nullable(v.pipe(v.string(), v.regex(/^[0-9a-f]{40,64}$/))),
		doc: v.optional(ModesDocSchema),
		text: v.optional(Str(TEXT_MAX)),
		message: v.optional(v.pipe(v.string(), v.trim(), v.maxLength(200), v.regex(/^[^\r\n]*$/)))
	}),
	v.check((i) => (i.doc === undefined) !== (i.text === undefined), 'Send the form or the TOML text')
);
