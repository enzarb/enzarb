<script lang="ts">
	import { untrack } from 'svelte';
	import Button from '../form/Button.svelte';
	import Select from '../form/Select.svelte';
	import TextArea from '../form/TextArea.svelte';
	import TextField from '../form/TextField.svelte';
	import SaveSteps from '../modes/SaveSteps.svelte';
	import Notice from '../ui/Notice.svelte';
	import Section from '../ui/Section.svelte';
	import Tabs from '../ui/Tabs.svelte';
	import type { StepItem } from '../ui/StepList.svelte';
	import DiagnosticList from './DiagnosticList.svelte';
	import DryRunPanel, { type DryRunApi } from './DryRunPanel.svelte';
	import NodeForm from './NodeForm.svelte';
	import WorkflowForm from './WorkflowForm.svelte';
	import WorkflowGraph from './WorkflowGraph.svelte';
	import WorkflowList from './WorkflowList.svelte';
	import {
		blankNode,
		blankWorkflow,
		exampleWorkflow,
		freeId,
		fromWire,
		hasErrorDiagnostics,
		nodesWithErrors,
		NODE_TYPES,
		removeNode,
		renameNode,
		toWire,
		type Diagnostic,
		type NodeType,
		type Trace,
		type WorkflowsDoc,
		type WorkflowsFile,
		type WorkflowsFileWire
	} from '#lib/workflows-types.ts';

	type Fail = { ok: false; message: string; code?: string; issues?: Diagnostic[] };
	export type WorkflowsApi = {
		/** The graph as TOML with tracks' report. */
		render: (
			file: WorkflowsFileWire
		) => Promise<{ ok: true; text: string; diagnostics: Diagnostic[] } | Fail>;
		/** TOML to the graph (null when it does not parse) with tracks' report. */
		check: (
			text: string
		) => Promise<{ ok: true; diagnostics: Diagnostic[]; file: WorkflowsFileWire | null } | Fail>;
		save: (input: {
			file?: WorkflowsFileWire;
			text?: string;
			message: string;
		}) => Promise<{ ok: true; unchanged: boolean; commitUrl: string | null } | Fail>;
		reload: () => void;
	};

	/**
	 * The workflow designer for one repository's `.ez/workflows.toml`: each workflow as a graph
	 * with a side panel per node, or the raw TOML; checked by tracks as the user edits, dry-run
	 * against logged events, and committed to the default branch (which makes it live).
	 */
	let {
		file,
		api,
		dryRun,
		saved
	}: {
		file: WorkflowsFile;
		api: WorkflowsApi;
		dryRun: DryRunApi;
		saved: (r: { unchanged: boolean; commitUrl: string | null }) => void;
	} = $props();

	const initial = untrack(() => file);
	const VALIDATE_DEBOUNCE_MS = 400;
	const savedWire = JSON.stringify(initial.file ?? { version: 1 });
	const hasComments = /^\s*#/m.test(initial.text ?? '');
	const repoName = initial.repo.replace(/^github:/, '');

	let doc = $state<WorkflowsDoc>(fromWire(initial.file ?? { version: 1 }));
	let diagnostics = $state<Diagnostic[]>(initial.report);
	let tab = $state(initial.file ? 'graph' : 'toml');
	let text = $state(initial.text ?? '');
	let wfIndex = $state(0);
	let nodeIndex = $state<number | null>(null);
	let newType = $state<NodeType>('agent');
	let highlight = $state<{ workflow: string; visited: string[]; taken: string[] } | null>(null);
	let switchError = $state('');
	let switching = $state(false);
	let message = $state('');
	let steps = $state<StepItem[]>([]);
	let saving = $state(false);
	let conflict = $state(false);

	const wf = $derived(doc.workflows[wfIndex]);
	const dirty = $derived(
		tab === 'graph'
			? initial.text === null
				? doc.workflows.length > 0
				: JSON.stringify(toWire(doc)) !== savedWire
			: text !== (initial.text ?? '')
	);

	// Check as the user edits (debounced; a newer edit wins).
	let seq = 0;
	$effect(() => {
		const mine = ++seq;
		const run =
			tab === 'graph'
				? (() => {
						const wire = toWire($state.snapshot(doc) as WorkflowsDoc);
						return () => api.render(wire);
					})()
				: (() => {
						const t = text;
						return () => api.check(t);
					})();
		const id = setTimeout(async () => {
			const r = await run().catch(() => null);
			if (r?.ok && mine === seq) diagnostics = r.diagnostics;
		}, VALIDATE_DEBOUNCE_MS);
		return () => clearTimeout(id);
	});

	async function currentText(): Promise<{ ok: true; text: string } | Fail> {
		if (tab === 'toml') return { ok: true, text };
		const r = await api.render(toWire($state.snapshot(doc) as WorkflowsDoc));
		return r.ok ? { ok: true, text: r.text } : r;
	}

	async function switchTab(next: string) {
		if (next === tab || switching) return;
		switchError = '';
		switching = true;
		try {
			if (next === 'toml') {
				const r = await api.render(toWire($state.snapshot(doc) as WorkflowsDoc));
				if (!r.ok) {
					switchError = r.message;
					return;
				}
				text = r.text;
				diagnostics = r.diagnostics;
			} else {
				const r = await api.check(text);
				if (!r.ok || !r.file) {
					switchError = `The TOML cannot be opened as a graph: ${
						r.ok ? (r.diagnostics[0]?.message ?? 'it does not parse') : r.message
					}`;
					if (r.ok) diagnostics = r.diagnostics;
					return;
				}
				doc = fromWire(r.file);
				diagnostics = r.diagnostics;
				wfIndex = Math.min(wfIndex, Math.max(0, doc.workflows.length - 1));
				nodeIndex = null;
			}
			tab = next;
		} catch {
			switchError = 'The workflows service did not answer. Try again.';
		} finally {
			switching = false;
		}
	}

	function addWorkflow(example: boolean) {
		const id = freeId(
			doc.workflows.map((w) => w.id),
			example ? 'fix-issue' : 'workflow'
		);
		doc.workflows.push(example ? exampleWorkflow(id) : blankWorkflow(id));
		wfIndex = doc.workflows.length - 1;
		nodeIndex = null;
	}

	function selectNode(id: string) {
		const i = wf?.nodes.findIndex((n) => n.id === id) ?? -1;
		nodeIndex = i >= 0 ? i : null;
	}

	function addNode() {
		if (!wf) return;
		const id = freeId(
			wf.nodes.map((n) => n.id),
			newType
		);
		wf.nodes.push(blankNode(id, newType));
		nodeIndex = wf.nodes.length - 1;
	}

	function selectTarget(t: { workflow: string; node: string | null }) {
		const i = doc.workflows.findIndex((w) => w.id === t.workflow);
		if (i < 0) return;
		if (tab !== 'graph') return;
		wfIndex = i;
		nodeIndex = null;
		if (t.node) selectNode(t.node);
	}

	function showTrace(trace: Trace) {
		const i = doc.workflows.findIndex((w) => w.id === trace.workflow);
		if (i >= 0 && tab === 'graph') {
			wfIndex = i;
			nodeIndex = null;
		}
		highlight = {
			workflow: trace.workflow,
			visited: trace.steps.map((s) => s.node),
			taken: trace.steps.flatMap((s) =>
				s.transition?.kind === 'moved' ? [`${s.transition.from}:${s.transition.edge}`] : []
			)
		};
	}

	function setStep(id: string, status: StepItem['status'], extra: Partial<StepItem> = {}) {
		steps = steps.map((s) => (s.id === id ? { ...s, status, ...extra } : s));
	}

	async function save() {
		saving = true;
		conflict = false;
		steps = [
			{ id: 'validate', status: 'active', label: 'Check the workflows' },
			{ id: 'commit', status: 'pending', label: `Commit ${initial.path} to ${initial.branch}` }
		];
		try {
			const fromGraph = tab === 'graph';
			const wire = toWire($state.snapshot(doc) as WorkflowsDoc);
			const check = fromGraph ? await api.render(wire) : await api.check(text);
			if (!check.ok || hasErrorDiagnostics(check.diagnostics)) {
				if (check.ok) diagnostics = check.diagnostics;
				setStep('validate', 'failed', {
					message: check.ok
						? 'Fix the errors listed under Validation: saving makes the workflows live.'
						: check.message
				});
				return;
			}
			diagnostics = check.diagnostics;
			setStep('validate', 'done');
			setStep('commit', 'active');
			const r = await api.save(fromGraph ? { file: wire, message } : { text, message });
			if (!r.ok) {
				conflict = r.code === 'conflict';
				if (r.issues) diagnostics = r.issues;
				setStep('commit', 'failed', { message: r.message });
				return;
			}
			setStep('commit', 'done', {
				message: r.unchanged ? 'Nothing changed: no commit was made.' : undefined
			});
			saved({ unchanged: r.unchanged, commitUrl: r.commitUrl });
		} catch {
			steps = steps.map((s) =>
				s.status === 'active'
					? { ...s, status: 'failed', message: 'The request failed. Try again.' }
					: s
			);
		} finally {
			saving = false;
		}
	}
</script>

<div class="ez-stack">
	<p class="ez-muted">
		<span class="ez-mono">{initial.path}</span> on <span class="ez-mono">{repoName}</span>, branch
		<span class="ez-mono">{initial.branch}</span>. Saving commits straight to that branch; runs use
		the committed file, so a file with errors is never saved. enzarb does not start runs yet: test
		your workflows with a dry run.
		{#if initial.modesToml === null}
			The branch has no <span class="ez-mono">.ez/modes.toml</span>, so agent nodes have no modes to
			use yet.
		{/if}
	</p>

	{#if initial.text !== null && !initial.file}
		<Notice tone="warning" title="The file does not parse as workflows">
			Fix it in the TOML tab (see Validation), then switch to the graph.
		</Notice>
	{/if}

	<Tabs
		label="Editor"
		tabs={[
			{ value: 'graph', label: 'Graph', disabled: switching },
			{ value: 'toml', label: 'TOML', disabled: switching }
		]}
		bind:value={() => tab, (v) => void switchTab(v)}
	>
		{#snippet panel(value)}
			{#if switchError}<Notice tone="error">{switchError}</Notice>{/if}
			{#if value === 'graph'}
				{#if !doc.workflows.length}
					<Section title="Start your workflows">
						<p class="ez-muted">
							{initial.text === null
								? `There is no ${initial.path} on ${initial.branch} yet.`
								: 'The file defines no workflows.'}
							Start from the example (triage issues labelled ez, then fix them or ask for details) or
							a blank workflow.
						</p>
						<div class="ez-cluster">
							<Button onclick={() => addWorkflow(true)}>Start from the example</Button>
							<Button onclick={() => addWorkflow(false)}>Blank workflow</Button>
						</div>
					</Section>
				{:else}
					{#if hasComments}
						<p class="ez-muted">
							Saving from the graph rewrites the file and drops its comments; edit in the TOML tab
							to keep them.
						</p>
					{/if}
					<div class="ez-wf-layout">
						<WorkflowList
							workflows={doc.workflows}
							bind:selected={
								() => wfIndex,
								(i) => {
									wfIndex = i;
									nodeIndex = null;
								}
							}
							{diagnostics}
							add={addWorkflow}
						/>
						{#if wf}
							<div class="ez-stack ez-wf-canvas">
								<WorkflowGraph
									workflow={wf}
									selected={nodeIndex === null ? null : (wf.nodes[nodeIndex]?.id ?? null)}
									errors={nodesWithErrors(diagnostics, wf.id)}
									visited={highlight?.workflow === wf.id ? highlight.visited : []}
									taken={highlight?.workflow === wf.id ? highlight.taken : []}
									select={selectNode}
								/>
								<div class="ez-cluster ez-cluster--end">
									<Select
										label="New node type"
										options={NODE_TYPES.map((t) => ({ value: t, label: t }))}
										bind:value={() => newType, (v) => (newType = v as NodeType)}
									/>
									<Button onclick={addNode}>Add node</Button>
									{#if nodeIndex !== null}
										<Button onclick={() => (nodeIndex = null)}>Workflow settings</Button>
									{/if}
									{#if highlight?.workflow === wf.id}
										<Button onclick={() => (highlight = null)}>Clear the trace</Button>
									{/if}
								</div>
							</div>
							<div class="ez-wf-panel">
								{#if nodeIndex !== null && wf.nodes[nodeIndex]}
									{#key `${wfIndex}:${nodeIndex}`}
										<NodeForm
											bind:node={wf.nodes[nodeIndex]}
											workflow={wf.id}
											targets={wf.nodes.map((n) => n.id)}
											modeIds={initial.modeIds}
											{diagnostics}
											isStart={wf.start === wf.nodes[nodeIndex].id}
											rename={(id) => {
												const w = doc.workflows[wfIndex];
												doc.workflows[wfIndex] = renameNode(w, w.nodes[nodeIndex!].id, id);
											}}
											remove={() => {
												const w = doc.workflows[wfIndex];
												doc.workflows[wfIndex] = removeNode(w, w.nodes[nodeIndex!].id);
												nodeIndex = null;
											}}
											makeStart={() => (wf.start = wf.nodes[nodeIndex!].id)}
										/>
									{/key}
								{:else}
									{#key wfIndex}
										<WorkflowForm
											bind:workflow={doc.workflows[wfIndex]}
											{diagnostics}
											rename={(id) => (doc.workflows[wfIndex].id = id)}
											remove={() => {
												doc.workflows.splice(wfIndex, 1);
												wfIndex = Math.max(0, wfIndex - 1);
												nodeIndex = null;
											}}
										/>
									{/key}
								{/if}
							</div>
						{/if}
					</div>
				{/if}
			{:else if value === 'toml'}
				<TextArea
					label={initial.path}
					class="ez-control ez-mono"
					rows={24}
					spellcheck={false}
					bind:value={text}
				/>
			{/if}
		{/snippet}
	</Tabs>

	<Section title="Validation">
		<DiagnosticList {diagnostics} select={selectTarget} />
	</Section>

	<Section title="Dry run">
		<DryRunPanel api={dryRun} workflows={doc.workflows} {currentText} show={showTrace} />
	</Section>

	<Section title="Save">
		<div class="ez-stack">
			<TextField
				label="Commit message"
				placeholder="chore(ez): update .ez/workflows.toml"
				maxlength={200}
				bind:value={message}
				disabled={saving}
			/>
			<div class="ez-cluster">
				<Button variant="primary" disabled={!dirty || saving || switching} onclick={save}>
					{saving ? 'Saving…' : `Commit to ${initial.branch}`}
				</Button>
				{#if conflict}<Button onclick={api.reload}>Reload the file</Button>{/if}
			</div>
			<SaveSteps label="Saving the workflows" items={steps} />
		</div>
	</Section>
</div>
