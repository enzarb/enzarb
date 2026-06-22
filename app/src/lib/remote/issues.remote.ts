import { query, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { z } from 'zod/v4';
import { listIssues, getIssue, createIssue, editIssue, listIssueComments, createIssueComment } from '$lib/gitea';
import { resolveOrg, requirePrivilege } from './guard';

export const getIssues = query(
	z.object({ state: z.enum(['open', 'closed', 'all']).default('open'), page: z.number().default(1) }),
	async ({ state, page }) => {
		const { params } = getRequestEvent();
		const org = resolveOrg();
		return (await listIssues(org.slug, params.project!, state, page)) ?? [];
	}
);

export const getIssueDetail = query(
	z.number(),
	async (index) => {
		const { params } = getRequestEvent();
		const org = resolveOrg();
		const [issue, comments] = await Promise.all([
			getIssue(org.slug, params.project!, index),
			listIssueComments(org.slug, params.project!, index)
		]);
		return { issue, comments: comments ?? [] };
	}
);

export const createIssueCmd = command(
	z.object({ title: z.string().min(1).max(255), body: z.string().default('') }),
	async ({ title, body }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');
		const result = await createIssue(org.slug, params.project!, title, body);
		await getIssues({ state: 'open', page: 1 }).refresh();
		return result;
	}
);

export const closeIssueCmd = command(
	z.object({ index: z.number() }),
	async ({ index }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');
		await editIssue(org.slug, params.project!, index, { state: 'closed' });
		await Promise.all([
			getIssues({ state: 'open', page: 1 }).refresh(),
			getIssueDetail(index).refresh()
		]);
	}
);

export const reopenIssueCmd = command(
	z.object({ index: z.number() }),
	async ({ index }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');
		await editIssue(org.slug, params.project!, index, { state: 'open' });
		await Promise.all([
			getIssues({ state: 'open', page: 1 }).refresh(),
			getIssueDetail(index).refresh()
		]);
	}
);

export const addCommentCmd = command(
	z.object({ index: z.number(), body: z.string().min(1) }),
	async ({ index, body }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');
		await createIssueComment(org.slug, params.project!, index, body);
		await getIssueDetail(index).refresh();
	}
);
