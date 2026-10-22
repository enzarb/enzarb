<script lang="ts">
	import { SvelteSet } from 'svelte/reactivity';
	import Button from '../form/Button.svelte';
	import Checkbox from '../form/Checkbox.svelte';
	import Select from '../form/Select.svelte';
	import Badge from '../ui/Badge.svelte';
	import Notice from '../ui/Notice.svelte';
	import TraceView from './TraceView.svelte';
	import type { EventRow } from '#lib/events.ts';
	import type {
		Diagnostic,
		DryRunOptions,
		DryRunResult,
		Trace,
		WorkflowDoc
	} from '#lib/workflows-types.ts';

	type Fail = { ok: false; message: string };
	export type DryRunApi = {
		/** Events logged for the repository, newest first; `before` pages. */
		events: (before?: number) => Promise<EventRow[]>;
		run: (input: { text: string; eventIds: number[]; options?: DryRunOptions }) => Promise<
			| {
					ok: true;
					report: { diagnostics: Diagnostic[] };
					results: DryRunResult[];
					missing: number[];
			  }
			| Fail
		>;
	};

	/**
	 * Dry runs: pick logged events, see which workflows they trigger and where each run would go.
	 * Runs the editor's current (unsaved) workflows; nothing is executed, posted or pushed.
	 */
	let {
		api,
		workflows,
		currentText,
		show
	}: {
		api: DryRunApi;
		workflows: readonly WorkflowDoc[];
		/** The editor's workflows as TOML (renders the graph when it is open). */
		currentText: () => Promise<{ ok: true; text: string } | Fail>;
		show: (trace: Trace) => void;
	} = $props();

	const MAX = 50;
	let events = $state<EventRow[]>([]);
	let loaded = $state(false);
	let more = $state(true);
	let loading = $state(false);
	let loadError = $state('');
	const chosen = new SvelteSet<number>();
	let forced = $state('');
	let running = $state(false);
	let error = $state('');
	let missing = $state<number[]>([]);
	let entries = $state<
		{ event: EventRow; result: DryRunResult; assume: Record<string, unknown> }[]
	>([]);

	async function load() {
		loading = true;
		loadError = '';
		try {
			const page = await api.events(events.at(-1)?.id);
			events = [...events, ...page];
			more = page.length > 0;
			loaded = true;
		} catch {
			loadError = 'Could not load the events.';
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		if (!loaded && !loading && !loadError) void load();
	});

	const label = (e: EventRow) =>
		`#${e.id} ${e.event}${e.action ? `.${e.action}` : ''}${e.summary ? `: ${e.summary}` : ''}`;
	const options = (assume: Record<string, unknown>): DryRunOptions => ({
		...(forced ? { workflow: forced } : {}),
		...(Object.keys(assume).length ? { assume } : {})
	});

	async function run() {
		running = true;
		error = '';
		missing = [];
		try {
			const t = await currentText();
			if (!t.ok) {
				error = t.message;
				return;
			}
			const ids = events.filter((e) => chosen.has(e.id)).map((e) => e.id);
			const r = await api.run({ text: t.text, eventIds: ids, options: options({}) });
			if (!r.ok) {
				error = r.message;
				return;
			}
			if (!r.results.length) {
				error = 'The workflows have errors (see Validation); fix them to dry-run.';
				entries = [];
				return;
			}
			missing = r.missing;
			const byId = new Map(events.map((e) => [e.id, e]));
			entries = r.results.flatMap((result) => {
				const event = byId.get(result.event_id);
				return event ? [{ event, result, assume: {} }] : [];
			});
		} catch {
			error = 'The dry run failed. Try again.';
		} finally {
			running = false;
		}
	}

	/** Re-runs one event with `next` as its assumed outcomes. */
	async function rerun(i: number, next: Record<string, unknown>) {
		const entry = entries[i];
		running = true;
		error = '';
		try {
			const t = await currentText();
			if (!t.ok) {
				error = t.message;
				return;
			}
			const r = await api.run({ text: t.text, eventIds: [entry.event.id], options: options(next) });
			if (!r.ok) error = r.message;
			else if (r.results[0]) entries[i] = { ...entry, result: r.results[0], assume: next };
			else error = 'The workflows have errors (see Validation); fix them to dry-run.';
		} catch {
			error = 'The dry run failed. Try again.';
		} finally {
			running = false;
		}
	}

	const assume = (i: number, node: string, outputs: unknown) =>
		rerun(i, { ...entries[i].assume, [node]: outputs });
</script>

<div class="ez-stack">
	<p class="ez-muted">
		Runs the workflows as they are in the editor, saved or not, against events this repository
		already sent. Nothing runs in a sandbox and nothing is posted to GitHub: agent, command, action
		and wait nodes stop the walk until you assume their outcome.
	</p>

	{#if loadError}
		<Notice tone="error">
			{loadError}
			{#snippet actions()}<Button onclick={load}>Retry</Button>{/snippet}
		</Notice>
	{:else if loaded && !events.length}
		<p class="ez-muted">No events were logged for this repository yet.</p>
	{:else if loaded}
		<fieldset class="ez-fieldset">
			<legend class="ez-fieldset__legend">Events ({chosen.size} of at most {MAX} chosen)</legend>
			<div class="ez-wf-events">
				{#each events as e (e.id)}
					<Checkbox
						label={label(e)}
						disabled={!chosen.has(e.id) && chosen.size >= MAX}
						bind:checked={
							() => chosen.has(e.id), (on) => (on ? chosen.add(e.id) : chosen.delete(e.id))
						}
					/>
				{/each}
			</div>
		</fieldset>
		<div class="ez-cluster">
			{#if more}<Button size="sm" disabled={loading} onclick={load}>Load older events</Button>{/if}
			<Button size="sm" disabled={!chosen.size} onclick={() => chosen.clear()}>Clear</Button>
		</div>
	{:else}
		<p class="ez-muted">Loading events…</p>
	{/if}

	<div class="ez-cluster ez-cluster--end">
		<div class="ez-max-select">
			<Select
				label="Also trace"
				help="Walk this workflow even when its trigger does not match."
				options={[
					{ value: '', label: 'Only matching workflows' },
					...workflows.map((w) => ({ value: w.id, label: w.id }))
				]}
				bind:value={forced}
			/>
		</div>
		<Button variant="primary" disabled={!chosen.size || running} onclick={run}>
			{running ? 'Running…' : 'Dry run'}
		</Button>
	</div>

	{#if error}<Notice tone="error">{error}</Notice>{/if}
	{#if missing.length}
		<Notice tone="warning">
			{missing.length === 1 ? 'One event was' : `${missing.length} events were`} not found for this repository
			and skipped.
		</Notice>
	{/if}

	{#each entries as entry, i (entry.event.id)}
		<section class="ez-wf-result" aria-label="Dry run of event {entry.event.id}">
			<h3 class="ez-wf-result__title">{label(entry.event)}</h3>
			<ul class="ez-wf-triggers" aria-label="Triggers">
				{#each entry.result.triggers as t (t.workflow)}
					<li>
						<Badge tone={t.matched ? 'success' : 'neutral'}
							>{t.matched ? 'matches' : 'no match'}</Badge
						>
						<span class="ez-mono">{t.workflow}</span>
						<span class="ez-muted">{t.reason}</span>
					</li>
				{/each}
			</ul>
			{#if !entry.result.traces.length}
				<p class="ez-muted">No workflow runs for this event.</p>
			{/if}
			{#each entry.result.traces as trace (trace.workflow)}
				<TraceView
					{trace}
					nodes={workflows.find((w) => w.id === trace.workflow)?.nodes ?? []}
					busy={running}
					assume={(node, outputs) => assume(i, node, outputs)}
					show={() => show(trace)}
				/>
			{/each}
			{#if Object.keys(entry.assume).length}
				<div>
					<Button size="sm" disabled={running} onclick={() => rerun(i, {})}
						>Forget the assumed outcomes</Button
					>
				</div>
			{/if}
		</section>
	{/each}
</div>
