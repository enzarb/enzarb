<script lang="ts">
	import { getIssueDetail, closeIssueCmd, reopenIssueCmd, addCommentCmd } from '$lib/remote/issues.remote';
	import { page } from '$app/stores';

	let commentBody = $state('');
	let submitting = $state(false);
	let commentError = $state('');

	$effect(() => { commentBody; });

	const issueNumber = $derived(Number($page.params.number));

	async function handleComment(e: SubmitEvent) {
		e.preventDefault();
		submitting = true;
		commentError = '';
		try {
			await addCommentCmd({ index: issueNumber, body: commentBody });
			commentBody = '';
			(e.target as HTMLFormElement).reset();
		} catch (err) {
			commentError = err instanceof Error ? err.message : 'Failed to add comment';
		} finally {
			submitting = false;
		}
	}

	async function handleClose() {
		await closeIssueCmd({ index: issueNumber });
	}

	async function handleReopen() {
		await reopenIssueCmd({ index: issueNumber });
	}

	function fmtDate(s: string) {
		return new Date(s).toLocaleString();
	}
</script>

{#await getIssueDetail(issueNumber) then { issue, comments }}
	<div class="issue-detail">
		<div class="issue-header">
			<a href="/{$page.params.namespace}/projects/{$page.params.project}/issues" class="back">← Issues</a>
			<div class="issue-title-row">
				<h2 class="issue-title">{issue.title} <span class="issue-num muted">#{issue.number}</span></h2>
				<span class="badge {issue.state}">{issue.state}</span>
			</div>
			<p class="issue-meta muted">
				Opened {fmtDate(issue.created_at)} by {issue.user?.login ?? '?'}
			</p>
		</div>

		{#if issue.body}
			<div class="card issue-body">{issue.body}</div>
		{/if}

		{#if comments.length > 0}
			<div class="comments">
				{#each comments as comment}
					<div class="card comment">
						<div class="comment-header muted">
							<strong>{comment.user?.login ?? '?'}</strong> &middot; {fmtDate(comment.created_at)}
						</div>
						<div class="comment-body">{comment.body}</div>
					</div>
				{/each}
			</div>
		{/if}

		<div class="card comment-form">
			<form onsubmit={handleComment}>
				<div class="field">
					<label for="comment-body">Add a comment</label>
					<textarea id="comment-body" name="body" class="input" rows="4" placeholder="Leave a comment…" bind:value={commentBody} required></textarea>
				</div>
				{#if commentError}<p class="err">{commentError}</p>{/if}
				<div class="form-actions">
					{#if issue.state === 'open'}
						<button type="button" class="btn btn-danger" onclick={handleClose}>Close issue</button>
					{:else}
						<button type="button" class="btn" onclick={handleReopen}>Reopen issue</button>
					{/if}
					<button type="submit" class="btn btn-primary" disabled={submitting || !commentBody.trim()}>
						{submitting ? 'Commenting…' : 'Comment'}
					</button>
				</div>
			</form>
		</div>
	</div>
{:catch err}
	<p class="muted">Could not load issue: {err?.message ?? 'unknown error'}</p>
{/await}

<style>
	.issue-detail { display: flex; flex-direction: column; gap: 1rem; max-width: 800px; }
	.back { font-size: 12px; color: var(--color-text-muted); display: block; margin-bottom: 0.5rem; }
	.issue-header { display: flex; flex-direction: column; gap: 0.25rem; }
	.issue-title-row { display: flex; align-items: flex-start; gap: 0.75rem; }
	.issue-title { font-size: 20px; font-weight: 600; margin: 0; }
	.issue-num { font-weight: 400; }
	.issue-meta { font-size: 12px; }
	.badge { padding: 0.2rem 0.5rem; border-radius: 999px; font-size: 11px; font-weight: 600; flex-shrink: 0; }
	.badge.open { background: #1a4d1a; color: #5cb85c; }
	.badge.closed { background: #4d1a1a; color: #c0392b; }
	.issue-body { white-space: pre-wrap; font-size: 13px; }
	.comments { display: flex; flex-direction: column; gap: 0.75rem; }
	.comment-header { font-size: 12px; margin-bottom: 0.4rem; }
	.comment-body { font-size: 13px; white-space: pre-wrap; }
	.comment-form { padding-top: 0; }
	.field { margin-bottom: 0.75rem; }
	label { display: block; font-size: 13px; font-weight: 500; margin-bottom: 0.25rem; }
	.input { width: 100%; padding: 0.4rem 0.6rem; border: 1px solid var(--color-border); border-radius: var(--radius); background: var(--color-bg); color: var(--color-text); font-size: 13px; font-family: inherit; box-sizing: border-box; resize: vertical; }
	.form-actions { display: flex; gap: 0.5rem; justify-content: flex-end; }
	.btn-danger { background: var(--color-danger, #c0392b); color: white; border-color: transparent; }
	.err { color: var(--color-danger, #c0392b); font-size: 12px; }
	.muted { color: var(--color-text-muted); }
</style>
