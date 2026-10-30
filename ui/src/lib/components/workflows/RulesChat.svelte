<script lang="ts" module>
	import type { ProposalDeps } from '#lib/rules-proposal.ts';

	/** What the proposal needs of the server: tracks validation of the agent's files. */
	export type RulesChatApi = Pick<ProposalDeps, 'parseModes' | 'checkRules'>;
</script>

<script lang="ts">
	import { untrack } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import type { AcpSession, ConnectionDeps } from '#lib/acp/connection.ts';
	import type { ChatItem } from '#lib/acp/types.ts';
	import Chat from '../chat/Chat.svelte';
	import Notice from '../ui/Notice.svelte';
	import type { StepItem } from '../ui/StepList.svelte';
	import RulesProposal from './RulesProposal.svelte';
	import type { TemplatesApi } from './RulesView.svelte';
	import type { WorkflowsApi } from './WorkflowsDesigner.svelte';
	import {
		ezEditBase,
		FILE_READ_FEATURE,
		REVERT_MESSAGE,
		rulesContextMessage
	} from '#lib/rules-chat.ts';
	import { highlightedRules, type RulesDiff } from '#lib/rules-diff.ts';
	import { acceptSteps, buildProposal, type Proposal } from '#lib/rules-proposal.ts';
	import type { WorkflowsFile } from '#lib/workflows-types.ts';

	/**
	 * The chat docked beside the rules. The agent edits `.ez/*.toml` in its worktree; each finished
	 * edit is read back over the mux, validated by tracks and shown as a proposal. Accepting commits
	 * through the same calls as the rules view (modes first); nothing here can push.
	 */
	let {
		project,
		file,
		api,
		templates,
		check,
		saved,
		highlighted,
		connect
	}: {
		project: string;
		file: WorkflowsFile;
		api: WorkflowsApi;
		templates: TemplatesApi;
		check: RulesChatApi;
		saved: (r: { unchanged: boolean; commitUrl: string | null }) => void;
		/** Called with the ids of the rules the proposal adds or changes (empty: none). */
		highlighted: (ids: Set<string>) => void;
		connect?: ConnectionDeps['connect'];
	} = $props();

	let chat = $state<ReturnType<typeof Chat>>();
	let session: AcpSession | null = null;
	let proposal = $state<Proposal | null>(null);
	let problem = $state('');
	let steps = $state<StepItem[]>([]);
	let busy = $state(false);
	const seen = new SvelteSet<string>();
	let seq = 0;

	const diff = $derived<RulesDiff | null>(proposal?.diff ?? null);
	$effect(() => {
		highlighted(highlightedRules(diff));
	});

	const wrapFirst = (text: string) =>
		`${rulesContextMessage(
			untrack(() => file.text),
			untrack(() => file.modesToml)
		)}\n\n${text}`;

	async function readFile(path: string) {
		const s = session;
		if (!s?.readFile || !s.features.has(FILE_READ_FEATURE)) {
			throw new Error('This sandbox cannot show proposals yet: it needs an update.');
		}
		let timer: ReturnType<typeof setTimeout> | undefined;
		const timeout = new Promise<never>((_, rej) => {
			timer = setTimeout(() => rej(new Error('The sandbox did not answer.')), 15000);
		});
		try {
			return await Promise.race([s.readFile(path), timeout]);
		} finally {
			clearTimeout(timer);
		}
	}

	async function refresh(base: string) {
		const mine = ++seq;
		try {
			const cur = await templates.load();
			const r = await buildProposal(
				base,
				{ readFile, ...check },
				{ file: file.file, modes: cur.doc, modesToml: cur.text }
			);
			if (mine !== seq) return;
			problem = r.ok ? '' : r.message;
			if (r.ok) {
				proposal = r.proposal;
				steps = [];
			}
		} catch (e) {
			if (mine === seq) problem = e instanceof Error ? e.message : 'Could not read the change.';
		}
	}

	function onItems(items: ChatItem[]) {
		for (const it of items) {
			if (it.kind !== 'tool' || seen.has(it.toolCallId)) continue;
			const base = ezEditBase(it);
			if (base === null) continue;
			seen.add(it.toolCallId);
			void refresh(base);
		}
	}

	async function accept() {
		const p = proposal;
		if (!p) return;
		busy = true;
		steps = acceptSteps(p, file.branch, file.path);
		const set = (id: string, status: StepItem['status'], msg?: string) =>
			(steps = steps.map((s) => (s.id === id ? { ...s, status, message: msg } : s)));
		const fail = (id: string, msg: string) => set(id, 'failed', msg);
		try {
			const cur = await templates.load();
			let modesText: string | null | undefined;
			if (p.modes && p.modesChanged) {
				const m = await templates.renderModes(p.modes, cur.text);
				if (!m.ok) return fail('check', m.message);
				modesText = m.text;
			}
			if (p.rulesChanged) {
				const c = await api.render(p.file, modesText);
				if (!c.ok) return fail('check', c.message);
				const bad = c.diagnostics.find((d) => d.severity === 'error');
				if (bad) return fail('check', bad.message);
			}
			set('check', 'done');
			if (p.modes && p.modesChanged) {
				set('modes', 'active');
				const r = await templates.saveModes({
					doc: p.modes,
					baseSha: cur.sha,
					message: 'chore(ez): update modes from the rules chat'
				});
				if (!r.ok) return fail('modes', r.message);
				set('modes', 'done');
			}
			let result = { unchanged: true, commitUrl: null as string | null };
			if (p.rulesChanged) {
				set('rules', 'active');
				const r = await api.save({
					file: p.file,
					message: 'chore(ez): update rules from the rules chat'
				});
				if (!r.ok) return fail('rules', r.message);
				set('rules', 'done');
				result = { unchanged: r.unchanged, commitUrl: r.commitUrl };
			}
			proposal = null;
			saved(result);
		} catch {
			steps = steps.map((s) =>
				s.status === 'active' ? { ...s, status: 'failed', message: 'The request failed.' } : s
			);
		} finally {
			busy = false;
		}
	}

	function discard() {
		proposal = null;
		steps = [];
		chat?.say(REVERT_MESSAGE);
	}
</script>

<aside class="ez-rules-chat ez-stack" aria-label="Chat about rules">
	{#if problem}
		<Notice tone="warning" title="The agent's change cannot be used yet">{problem}</Notice>
	{/if}
	{#if proposal}
		<RulesProposal diff={proposal.diff} {steps} {busy} {accept} {discard} />
	{/if}
	{#key project}
		<Chat
			bind:this={chat}
			{project}
			{connect}
			{wrapFirst}
			{onItems}
			onSession={(s) => (session = s)}
		/>
	{/key}
</aside>
