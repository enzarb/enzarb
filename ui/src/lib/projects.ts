import * as v from 'valibot';
import type { Project, ProjectView, Repository } from './project-types.ts';

/** Namespaces are `enzarb-p-<name>` (contracts §2) and must stay within 63 characters. */
export const MAX_NAME_LENGTH = 54;

const NAME_RE = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/;
const GITHUB_URL_RE =
	/^https:\/\/github\.com\/([A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)\/([A-Za-z0-9._-]+)$/;

/** Repo key `<connector>:<owner>/<repo>`, lowercase (contracts §2). Undefined if not a canonical URL. */
export function repoKey(url: string, connector = 'github'): string | undefined {
	const m = GITHUB_URL_RE.exec(url);
	if (!m || m[2].endsWith('.git') || m[2] === '.' || m[2] === '..') return undefined;
	return `${connector}:${m[1]}/${m[2]}`.toLowerCase();
}

const Name = v.pipe(
	v.string(),
	v.trim(),
	v.minLength(1, 'Name is required'),
	v.maxLength(MAX_NAME_LENGTH, `Name must be at most ${MAX_NAME_LENGTH} characters`),
	v.regex(NAME_RE, 'Use lowercase letters, digits and "-"; start and end with a letter or digit')
);

const RepoUrl = v.pipe(
	v.string(),
	v.trim(),
	v.check(
		(u) => repoKey(u) !== undefined,
		'Use a canonical URL like https://github.com/owner/repo (no .git, no trailing slash)'
	)
);

const Branch = v.pipe(
	v.string(),
	v.trim(),
	v.minLength(1, 'Branch is required'),
	v.regex(/^[^\s~^:?*[\\]+$/, 'Invalid branch name')
);

const bool = v.optional(v.boolean(), false);

export const CreateProjectSchema = v.pipe(
	v.object({
		name: Name,
		displayName: v.optional(v.pipe(v.string(), v.trim(), v.maxLength(100)), ''),
		repositories: v.optional(
			v.array(v.object({ url: RepoUrl, defaultBranch: v.optional(Branch, 'main') })),
			[]
		),
		issues: bool,
		changeRequests: bool,
		mentions: bool,
		docker: bool,
		runtimeClassName: v.optional(v.pipe(v.string(), v.trim()), '')
	}),
	v.forward(
		v.check(
			(p) => !p.docker || p.runtimeClassName !== '',
			'Docker requires a runtime class (gvisor or kata)'
		),
		['runtimeClassName']
	),
	v.forward(
		v.check((p) => {
			const keys = p.repositories.map((r) => repoKey(r.url));
			return new Set(keys).size === keys.length;
		}, 'Each repository may be listed only once'),
		['repositories']
	)
);

export type CreateProjectInput = v.InferOutput<typeof CreateProjectSchema>;

/** Build the `Project` object for a validated form (contracts §2; owner = GitHub login). */
export function buildProject(input: CreateProjectInput, owner: string): Project {
	return {
		apiVersion: 'enzarb.dev/v1alpha1',
		kind: 'Project',
		metadata: { name: input.name },
		spec: {
			owner,
			displayName: input.displayName || input.name,
			repositories: input.repositories.map((r): Repository => ({
				url: r.url,
				connector: 'github',
				defaultBranch: r.defaultBranch
			})),
			runMode: 'worktree',
			triggers: {
				issues: input.issues,
				changeRequests: input.changeRequests,
				mentions: input.mentions
			},
			// Docker needs an isolating runtime class (ADR-0012); otherwise leave sandbox defaults alone.
			...(input.docker
				? { sandbox: { runtimeClassName: input.runtimeClassName, docker: { enabled: true } } }
				: {})
		}
	};
}

export function toView(p: Project): ProjectView {
	const conditions = p.status?.conditions ?? [];
	return {
		name: p.metadata.name,
		displayName: p.spec.displayName || p.metadata.name,
		namespace: p.status?.namespace ?? null,
		repositories: p.spec.repositories ?? [],
		triggers: {
			issues: p.spec.triggers?.issues ?? true,
			changeRequests: p.spec.triggers?.changeRequests ?? true,
			mentions: p.spec.triggers?.mentions ?? true
		},
		dockerEnabled: p.spec.sandbox?.docker?.enabled ?? false,
		runtimeClassName: p.spec.sandbox?.runtimeClassName ?? '',
		conditions,
		ready: conditions.some((c) => c.type === 'Ready' && c.status === 'True')
	};
}
