import * as v from 'valibot';
import { MAX_NAME_LENGTH, NameSchema } from './projects.ts';

/** How long a repository snapshot may authorize Project creation. */
export const SNAPSHOT_MAX_AGE_MS = 24 * 60 * 60 * 1000;
/** Upper bound for one bulk request. */
export const MAX_PROJECTS_PER_REQUEST = 50;

/** One Project to create: a name and the repository keys (`github:<owner>/<repo>`) it holds. */
export interface ProjectPlan {
	name: string;
	displayName?: string;
	repositories: string[];
	/** Concurrent agent runs; omitted for the sandbox default. */
	maxRuns?: number;
}

export interface Triggers {
	issues: boolean;
	changeRequests: boolean;
	mentions: boolean;
}

export type PlanResult =
	| { name: string; status: 'created'; repositories: string[] }
	| { name: string; status: 'failed'; repositories: string[]; reason: string };

export type CreateProjectsOutcome =
	| { ok: true; results: PlanResult[] }
	| { ok: false; code: 'no-snapshot' | 'stale-snapshot'; message: string };

/**
 * A DNS label from free text: lowercase `[a-z0-9-]`, no leading or trailing hyphen, at most
 * 50 characters. Empty when nothing usable remains.
 */
export function slugifyName(input: string): string {
	return input
		.normalize('NFKD')
		.replace(/\p{M}+/gu, '')
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+/, '')
		.slice(0, MAX_NAME_LENGTH)
		.replace(/-+$/, '');
}

/** `github:Owner/Repo` -> its parts, undefined for anything else. */
export function splitRepoKey(key: string): { owner: string; repo: string } | undefined {
	const m = /^github:([^/\s]+)\/([^/\s]+)$/.exec(key);
	return m ? { owner: m[1], repo: m[2] } : undefined;
}

/** Expands `{repo}` and `{owner}` in a name template, then slugifies the result. */
export function expandNameTemplate(template: string, parts: { owner: string; repo: string }) {
	return slugifyName(template.replaceAll('{repo}', parts.repo).replaceAll('{owner}', parts.owner));
}

/** The first problem with a Project name, or undefined when it is a valid name. */
export function validateProjectName(name: string): string | undefined {
	const r = v.safeParse(NameSchema, name);
	return r.success ? undefined : r.issues[0].message;
}

/**
 * Problems per plan (by index) that can be known without the cluster: invalid names, names or
 * repositories used twice in the request, names that already exist.
 */
export function planProblems(plans: ProjectPlan[], existingNames: ReadonlySet<string>): string[][] {
	const nameCount = new Map<string, number>();
	for (const p of plans) nameCount.set(p.name, (nameCount.get(p.name) ?? 0) + 1);
	return plans.map((p, index) => {
		const out: string[] = [];
		if (index === 0 && plans.length > MAX_PROJECTS_PER_REQUEST)
			out.push(`At most ${MAX_PROJECTS_PER_REQUEST} projects per request`);
		const bad = validateProjectName(p.name);
		if (bad) out.push(bad);
		else if (existingNames.has(p.name)) out.push(`A project named "${p.name}" already exists`);
		else if ((nameCount.get(p.name) ?? 0) > 1)
			out.push(`The name "${p.name}" is used more than once`);
		if (p.repositories.length === 0) out.push('Select at least one repository');
		return out;
	});
}
