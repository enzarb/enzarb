import { error } from '@sveltejs/kit';
import { query } from '$app/server';
import * as v from 'valibot';
import { parseWindow } from '#lib/metrics-window.ts';
import { getSql } from '#lib/server/db.ts';
import { projectUsage } from '#lib/server/metrics-service.ts';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';

const Param = v.optional(v.pipe(v.string(), v.maxLength(40)));

/**
 * Usage and cost of the project's tasks in a window (`window`, or `from` and `to`; see
 * `parseWindow`). The window is validated here again: the URL is user input.
 */
export const getProjectUsage = query(
	v.object({ name: NameArg, window: Param, from: Param, to: Param }),
	async ({ name, window, from, to }) => {
		await ownedProject(name);
		const params = new URLSearchParams();
		if (window !== undefined) params.set('window', window);
		if (from !== undefined) params.set('from', from);
		if (to !== undefined) params.set('to', to);
		const parsed = parseWindow(params, new Date());
		if (!parsed.ok) error(400, parsed.error.message);
		return projectUsage(getSql(), name, parsed.window);
	}
);
