<script lang="ts">
	import { untrack } from 'svelte';
	import Button from '../form/Button.svelte';
	import ListField from '../modes/ListField.svelte';
	import Select from '../form/Select.svelte';
	import TextField from '../form/TextField.svelte';
	import SaveSteps from '../modes/SaveSteps.svelte';
	import Badge from '../ui/Badge.svelte';
	import Notice from '../ui/Notice.svelte';
	import type { StepItem } from '../ui/StepList.svelte';
	import type { DryRunApi } from './DryRunPanel.svelte';
	import type { WorkflowsApi } from './WorkflowsDesigner.svelte';
	import type { EventRow } from '#lib/events.ts';
	import { eventLabel, KNOWN_EVENTS, workflowModes } from '#lib/rules.ts';
	import {
		fromWire,
		hasErrorDiagnostics,
		toWire,
		type WorkflowsDoc,
		type WorkflowsFile
	} from '#lib/workflows-types.ts';

	/**
	 * Workflows as rules: "When <events> and <if> Then run <workflow> Using mode <modes>". Edits
	 * the trigger only; the steps stay in the graph designer (`advanced`). Saves through the same
	 * render/save calls as the designer, so tracks validates every change.
	 */
	let {
		file,
		api,
		dryRun,
		saved,
		advanced
	}: {
		file: WorkflowsFile;
		api: WorkflowsApi;
		dryRun: DryRunApi;
		saved: (r: { unchanged: boolean; commitUrl: string | null }) => void;
		advanced: (workflowId: string) => void;
	} = $props();

	const initial = untrack(() => file);
	const savedWire = JSON.stringify(initial.file ?? { version: 1 });
	let doc = $state<WorkflowsDoc>(fromWire(initial.file ?? { version: 1 }));
	let message = $state('');
	let steps = $state<StepItem[]>([]);
	let saving = $state(false);
	let matches = $state<Record<string, EventRow[]>>({});
	let matchError = $state('');

	const dirty = $derived(
		JSON.stringify(toWire($state.snapshot(doc) as WorkflowsDoc)) !== savedWire
	);
	const options = KNOWN_EVENTS;

	// Recent logged events each saved rule would have started (dry run of the saved file).
	$effect(() => {
		const text = initial.text;
		if (!text || !initial.file) return;
		void (async () => {
			try {
				const events = (await dryRun.events()).slice(0, 25);
				if (!events.length) return;
				const r = await dryRun.run({ text, eventIds: events.map((e) => e.id) });
				if (!r.ok) return;
				const out: Record<string, EventRow[]> = {};
				for (const res of r.results) {
					const ev = events.find((e) => e.id === res.event_id);
					if (!ev) continue;
					for (const t of res.triggers) if (t.matched) (out[t.workflow] ??= []).push(ev);
				}
				matches = out;
			} catch {
				matchError = 'Could not check recent events against the rules.';
			}
		})();
	});

	const addOn = (i: number, v: string) => {
		const val = v.trim();
		if (val && !doc.workflows[i].on.includes(val)) doc.workflows[i].on.push(val);
	};

	async function save() {
		saving = true;
		steps = [
			{ id: 'validate', status: 'active', label: 'Check the rules' },
			{ id: 'commit', status: 'pending', label: `Commit ${initial.path} to ${initial.branch}` }
		];
		const set = (id: string, status: StepItem['status'], msg?: string) =>
			(steps = steps.map((s) => (s.id === id ? { ...s, status, message: msg } : s)));
		try {
			const wire = toWire($state.snapshot(doc) as WorkflowsDoc);
			const check = await api.render(wire);
			if (!check.ok || hasErrorDiagnostics(check.diagnostics)) {
				set(
					'validate',
					'failed',
					check.ok
						? (check.diagnostics.find((d) => d.severity === 'error')?.message ??
								'The rules have errors.')
						: check.message
				);
				return;
			}
			set('validate', 'done');
			set('commit', 'active');
			const r = await api.save({ file: wire, message });
			if (!r.ok) {
				set('commit', 'failed', r.message);
				return;
			}
			set('commit', 'done', r.unchanged ? 'Nothing changed: no commit was made.' : undefined);
			saved({ unchanged: r.unchanged, commitUrl: r.commitUrl });
		} catch {
			steps = steps.map((s) =>
				s.status === 'active' ? { ...s, status: 'failed', message: 'The request failed.' } : s
			);
		} finally {
			saving = false;
		}
	}
</script>

<div class="ez-stack">
	<p class="ez-muted">
		Each rule says <strong>When</strong> something happens on GitHub, <strong>Then run</strong> a
		workflow <strong>Using mode</strong> an agent preset. Use Advanced to edit a workflow's steps.
	</p>
	{#if !doc.workflows.length}
		<p class="ez-muted">There are no rules yet. Open Advanced to create a workflow.</p>
	{/if}
	{#if matchError}<Notice tone="warning">{matchError}</Notice>{/if}
	<ul class="ez-rules" aria-label="Rules">
		{#each doc.workflows as wf, i (wf.id)}
			<li class="ez-rules__item" aria-label="Rule {wf.id}">
				<p class="ez-rules__sentence">
					<strong>When</strong>
					{#each wf.on as ev, k (ev)}
						{#if k > 0}or{/if}
						<Badge plain>{eventLabel(ev)}</Badge>
					{/each}
					{#if wf.if.trim()}<strong>and</strong> <code class="ez-mono">{wf.if}</code>{/if}
					<strong>Then run</strong>
					{wf.description || wf.id}
					{#if workflowModes(wf).length}
						<strong>Using mode</strong>
						{#each workflowModes(wf) as m (m)}<Badge tone="accent">{m}</Badge>{/each}
					{/if}
				</p>
				<div class="ez-cluster ez-cluster--end">
					<ListField
						label="Events for {wf.id} (one per line)"
						help="issues.labeled, push, or any event.action"
						bind:value={wf.on}
					/>
					<Select
						label="Add event for {wf.id}"
						{options}
						placeholder="Choose an event…"
						bind:value={() => '', (v) => addOn(i, v)}
					/>
					<TextField label="Only if, for {wf.id}" class="ez-control ez-mono" bind:value={wf.if} />
					<Button size="sm" onclick={() => advanced(wf.id)}>Advanced</Button>
				</div>
				<p class="ez-muted">
					{#if matches[wf.id]?.length}
						Would have matched {matches[wf.id].length} recent
						{matches[wf.id].length === 1 ? 'event' : 'events'}:
						{matches[wf.id]
							.slice(0, 3)
							.map((e) => `#${e.id} ${e.event}${e.action ? `.${e.action}` : ''}`)
							.join(', ')}
					{:else}
						No recent event would have matched this rule.
					{/if}
				</p>
			</li>
		{/each}
	</ul>
	{#if doc.workflows.length}
		<TextField
			label="Commit message"
			placeholder="chore(ez): update .ez/workflows.toml from enzarb"
			maxlength={200}
			bind:value={message}
			disabled={saving}
		/>
		<div class="ez-cluster">
			<Button variant="primary" disabled={!dirty || saving} onclick={save}>
				{saving ? 'Saving…' : `Commit to ${initial.branch}`}
			</Button>
		</div>
		<SaveSteps label="Saving the rules" items={steps} />
	{/if}
</div>
