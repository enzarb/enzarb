import { error } from '@sveltejs/kit';
import { query } from '$app/server';
import * as v from 'valibot';
import { parseWindow } from '#lib/metrics-window.ts';
import { getConfig } from '#lib/server/config.ts';
import { getSql } from '#lib/server/db.ts';
import { projectStepTiming, projectUsage, sandboxStats } from '#lib/server/metrics-service.ts';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';

const Param = v.optional(v.pipe(v.string(), v.maxLength(40)));
const Args = v.object({ name: NameArg, window: Param, from: Param, to: Param });

/** Owner check and window validation shared by the metrics queries; the URL is user input. */
async function ownedWindow({ name, window, from, to }: v.InferOutput<typeof Args>) {
	await ownedProject(name);
	const params = new URLSearchParams();
	if (window !== undefined) params.set('window', window);
	if (from !== undefined) params.set('from', from);
	if (to !== undefined) params.set('to', to);
	const parsed = parseWindow(params, new Date());
	if (!parsed.ok) error(400, parsed.error.message);
	return parsed.window;
}

/**
 * Usage and cost of the project's tasks in a window (`window`, or `from` and `to`; see
 * `parseWindow`). The window is validated here again: the URL is user input.
 */
export const getProjectUsage = query(Args, async (args) =>
	projectUsage(getSql(), args.name, await ownedWindow(args))
);

/** Per-step timing (active and waiting) and job durations in the same window. */
export const getProjectStepTiming = query(Args, async (args) =>
	projectStepTiming(getSql(), args.name, await ownedWindow(args))
);

/** Sandbox CPU, memory, storage and disk I/O of the project's sandboxes, bucketed over the window. */
export const getSandboxStats = query(Args, async (args) =>
	sandboxStats(getSql(), args.name, await ownedWindow(args), getConfig().sandboxStatsRetentionDays)
);
