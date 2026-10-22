import * as v from 'valibot';
import { NameArg } from '#lib/server/owned-project.ts';
import { MAX_DRY_RUN_EVENTS } from '#lib/server/workflows-service.ts';
import type { WorkflowsFileWire } from '#lib/workflows-types.ts';

/**
 * Input schemas of the workflow designer's remote functions (`workflows.remote.ts` may export only
 * remote functions). The file is checked for shape and size only; tracks does the validation.
 */

export const TEXT_MAX = 256 * 1024;
const Text = v.pipe(v.string(), v.maxLength(TEXT_MAX));

/** The designer's wire document (`WorkflowsFileWire`): shape and size only, tracks checks the rest. */
export const WorkflowsFileSchema = v.custom<WorkflowsFileWire>((f) => {
	if (!f || typeof f !== 'object' || Array.isArray(f)) return false;
	const { version, workflows } = f as { version?: unknown; workflows?: unknown };
	if (typeof version !== 'number') return false;
	if (workflows !== undefined && (!workflows || typeof workflows !== 'object')) return false;
	return JSON.stringify(f).length <= TEXT_MAX * 2;
}, 'Not a workflows document, or too large');

const Repo = v.pipe(v.string(), v.maxLength(250));
const Sha = v.nullable(v.pipe(v.string(), v.regex(/^[0-9a-f]{40,64}$/)));

export const SaveWorkflowsSchema = v.pipe(
	v.object({
		name: NameArg,
		repo: Repo,
		baseSha: Sha,
		file: v.optional(WorkflowsFileSchema),
		text: v.optional(Text),
		message: v.optional(v.pipe(v.string(), v.trim(), v.maxLength(200), v.regex(/^[^\r\n]*$/)))
	}),
	v.check(
		(i) => (i.file === undefined) !== (i.text === undefined),
		'Send the graph or the TOML text'
	)
);

const Id = v.pipe(v.number(), v.integer(), v.minValue(1));

export const DryRunSchema = v.object({
	name: NameArg,
	repo: Repo,
	text: Text,
	modesToml: v.nullable(Text),
	eventIds: v.pipe(v.array(Id), v.minLength(1), v.maxLength(MAX_DRY_RUN_EVENTS)),
	options: v.optional(
		v.object({
			workflow: v.optional(v.pipe(v.string(), v.maxLength(64))),
			assume: v.optional(
				v.pipe(
					v.record(v.pipe(v.string(), v.maxLength(64)), v.unknown()),
					v.check(
						(r) => JSON.stringify(r).length <= 256 * 1024,
						'The assumed outputs are too large'
					)
				)
			),
			max_steps: v.optional(v.pipe(v.number(), v.integer(), v.minValue(0), v.maxValue(1000)))
		})
	)
});

export const RepoEventsSchema = v.object({
	name: NameArg,
	repo: Repo,
	before: v.optional(Id)
});
