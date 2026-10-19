import { error } from '@sveltejs/kit';
import { command, getRequestEvent, query } from '$app/server';
import * as v from 'valibot';
import { renderMarkdown } from '#lib/server/markdown.ts';
import { getSql } from '#lib/server/db.ts';
import { getProjectStore } from '#lib/server/kube.ts';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';
import { projectCache } from '#lib/server/project-cache.ts';
import { issueRealtimeConnection } from '#lib/server/realtime-connection.ts';
import {
	TaskError,
	chatsService,
	secretsService,
	tasksService
} from '#lib/server/tasks-service.ts';
import { SECRET_NAME, SubmitSchema } from '#lib/tasks.ts';

async function service() {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	const projects = await projectCache.list(getProjectStore(), user.login);
	return {
		user,
		tasks: tasksService(
			getSql(),
			projects.map((p) => p.name)
		)
	};
}

/** Maps service errors to HTTP errors. */
async function guard<T>(f: () => Promise<T>): Promise<T> {
	try {
		return await f();
	} catch (e) {
		if (e instanceof TaskError) error(e.kind === 'not-found' ? 404 : 400, e.message);
		throw e;
	}
}

const Id = v.pipe(v.number(), v.integer(), v.minValue(1));
const RunId = v.pipe(v.string(), v.minLength(1), v.maxLength(200));
const Seq = v.optional(v.pipe(v.number(), v.integer(), v.minValue(0)), 0);

/** Sets the HttpOnly realtime cookie for the session user; returns the stream URL, never a token. */
export const realtimeConnection = command(async () => issueRealtimeConnection(getRequestEvent()));

export const listTasks = query(v.optional(NameArg), async (project) =>
	(await service()).tasks.list(project)
);

export const listNeedsYou = query(async () => (await service()).tasks.needsYou());

export const listFindings = query(v.optional(NameArg), async (project) =>
	(await service()).tasks.findings(project)
);

export const getTask = query(Id, async (id) => {
	const row = await (await service()).tasks.get(id);
	if (!row) error(404, 'Task not found');
	return row;
});

export const getTaskDetail = query(Id, async (id) => {
	const d = await (await service()).tasks.detail(id);
	if (!d) error(404, 'Task not found');
	return {
		...d,
		description: d.description
			? { ...d.description, html: renderMarkdown(d.description.body) }
			: null,
		artifacts: d.artifacts.map((a) =>
			a.format === 'markdown' ? { ...a, html: renderMarkdown(a.content ?? '') } : a
		)
	};
});

export const getSession = query(
	v.object({ taskId: Id, runId: RunId, afterSeq: Seq }),
	async ({ taskId, runId, afterSeq }) =>
		guard(async () => (await service()).tasks.session(taskId, runId, afterSeq))
);

export const searchSessions = query(
	v.object({
		project: NameArg,
		query: v.pipe(v.string(), v.trim(), v.minLength(2), v.maxLength(200))
	}),
	async ({ project, query: q }) => guard(async () => (await service()).tasks.search(project, q))
);

export const getBoard = query(NameArg, async (project) =>
	guard(async () => (await service()).tasks.board(project))
);

export const getWorkflowView = query(
	v.object({
		project: NameArg,
		repo: v.pipe(v.string(), v.maxLength(200), v.regex(/^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/))
	}),
	async ({ project, repo }) => guard(async () => (await service()).tasks.workflow(project, repo))
);

/** Posts a UI event (a declared button, a comment, steer, stop, resume, cancel) as the user. */
export const submitTask = command(SubmitSchema, async (input) => {
	const { user, tasks } = await service();
	const data = 'data' in input ? input.data : {};
	await guard(() =>
		tasks.submit(input.id, { login: user.login, id: user.githubId }, input.kind, data)
	);
});

// ------------------------------------------------------------------- secrets

const SecretArg = v.pipe(v.string(), v.regex(SECRET_NAME, 'Use A-Z, 0-9 and _ (e.g. NPM_TOKEN)'));
const Scope = v.optional(NameArg);

/** Owner of a secret scope: the user, or a project the user owns. */
async function scopeOwner(project: string | undefined) {
	if (project === undefined) {
		const { user } = getRequestEvent().locals;
		if (!user) error(401, 'Not signed in');
		return { scope: 'user' as const, owner: user.login };
	}
	await ownedProject(project);
	return { scope: 'project' as const, owner: project };
}

export const listSecrets = query(Scope, async (project) => {
	const { scope, owner } = await scopeOwner(project);
	return secretsService(getSql()).list(scope, owner);
});

export const setSecret = command(
	v.object({
		project: Scope,
		name: SecretArg,
		value: v.pipe(v.string(), v.minLength(1), v.maxLength(16_384))
	}),
	async ({ project, name, value }) => {
		const { scope, owner } = await scopeOwner(project);
		await secretsService(getSql()).set(scope, owner, name, value);
		await listSecrets(project).refresh();
	}
);

export const deleteSecret = command(
	v.object({ project: Scope, name: SecretArg }),
	async ({ project, name }) => {
		const { scope, owner } = await scopeOwner(project);
		await secretsService(getSql()).remove(scope, owner, name);
		await listSecrets(project).refresh();
	}
);

// ------------------------------------------------------------------- chats

async function chats() {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	return chatsService(getSql(), user.login);
}

export const listChats = query(NameArg, async (project) => (await chats()).list(project));

export const getChatSession = query(v.object({ runId: RunId, afterSeq: Seq }), async (a) =>
	guard(async () => (await chats()).session(a.runId, a.afterSeq))
);
