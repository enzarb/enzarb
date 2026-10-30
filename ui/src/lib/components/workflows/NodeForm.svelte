<script lang="ts">
	import { untrack } from 'svelte';
	import Button from '../form/Button.svelte';
	import Checkbox from '../form/Checkbox.svelte';
	import Select from '../form/Select.svelte';
	import TextArea from '../form/TextArea.svelte';
	import TextField from '../form/TextField.svelte';
	import ListField from '../modes/ListField.svelte';
	import EdgesField from './EdgesField.svelte';
	import {
		ACTION_KINDS,
		diagnosticsAt,
		END_STATUSES,
		NODE_OUTPUTS,
		NODE_TYPES,
		type Diagnostic,
		type NodeDoc
	} from '#lib/workflows-types.ts';

	/**
	 * One node's settings. Fields of the other types are kept while the type changes, so switching
	 * back loses nothing; only the current type's fields are written to the file.
	 */
	let {
		node = $bindable(),
		workflow,
		targets,
		modeIds,
		diagnostics,
		isStart,
		rename,
		remove,
		makeStart
	}: {
		node: NodeDoc;
		/** The workflow id, to locate diagnostics (`workflows.<id>.nodes.<node>...`). */
		workflow: string;
		targets: string[];
		modeIds: string[];
		diagnostics: readonly Diagnostic[];
		isStart: boolean;
		rename: (id: string) => void;
		remove: () => void;
		makeStart: () => void;
	} = $props();

	const at = $derived(`workflows.${workflow}.nodes.${node.id}`);
	const issuesOf = (field: string) => diagnosticsAt(diagnostics, `${at}.${field}`);
	const typeOptions = NODE_TYPES.map((t) => ({ value: t, label: t }));
	const actionOptions = ACTION_KINDS.map((a) => ({ value: a, label: a.replaceAll('_', ' ') }));
	const statusOptions = END_STATUSES.map((s) => ({ value: s, label: s }));
	const modeOptions = $derived(
		[...modeIds, ...(node.mode && !modeIds.includes(node.mode) ? [node.mode] : [])].map((m) => ({
			value: m,
			label: modeIds.includes(m) ? m : `${m} (not in .ez/modes.toml)`
		}))
	);

	let resultText = $state(untrack(() => (node.result ? JSON.stringify(node.result, null, 2) : '')));
	let resultError = $state('');
	function onResultInput() {
		resultError = '';
		if (!resultText.trim()) {
			node.result = null;
			return;
		}
		try {
			const v = JSON.parse(resultText) as unknown;
			if (!v || typeof v !== 'object' || Array.isArray(v)) throw new Error('not an object');
			node.result = v as Record<string, unknown>;
		} catch {
			resultError = 'Not a JSON object; the last valid value is kept.';
		}
	}

	const visits = {
		get: () => (node.max_visits === null ? '' : String(node.max_visits)),
		set: (v: string) => {
			const n = Number.parseInt(v, 10);
			node.max_visits = Number.isFinite(n) && n >= 0 ? n : null;
		}
	};
</script>

<div class="ez-stack">
	<div class="ez-cluster ez-cluster--end">
		<TextField
			label="Node id"
			class="ez-control ez-mono"
			bind:value={() => node.id, (v) => rename(v.trim())}
			issues={diagnostics.filter((d) => d.path === at).map((d) => d.message)}
		/>
		<Select
			label="Type"
			options={typeOptions}
			bind:value={() => node.type, (v) => (node.type = v as NodeDoc['type'])}
		/>
	</div>
	<div class="ez-cluster">
		{#if isStart}
			<span class="ez-muted">Runs start here.</span>
		{:else}
			<Button size="sm" onclick={makeStart}>Make start node</Button>
		{/if}
		<Button size="sm" variant="danger" onclick={remove}>Remove node</Button>
	</div>

	{#if node.type === 'agent'}
		{#if modeIds.length}
			<Select
				label="Mode"
				options={modeOptions}
				bind:value={node.mode}
				placeholder="Choose a mode…"
				issues={issuesOf('mode')}
			/>
		{:else}
			<TextField
				label="Mode"
				help="A mode of .ez/modes.toml on the same branch (there is none yet)."
				class="ez-control ez-mono"
				bind:value={node.mode}
				issues={issuesOf('mode')}
			/>
		{/if}
		<TextArea
			label="Instruction"
			help="The agent's first message; a template over event, subject, nodes and run."
			rows={6}
			class="ez-control ez-mono"
			bind:value={node.instruction}
			issues={issuesOf('instruction')}
		/>
		<TextArea
			label="Result"
			help={'Optional JSON object: property name → JSON Schema, e.g. {"decision": {"enum": ["fix", "ask"]}}. The agent must submit it; edges read it as result.'}
			rows={4}
			class="ez-control ez-mono"
			spellcheck={false}
			bind:value={resultText}
			oninput={onResultInput}
			issues={[...(resultError ? [resultError] : []), ...issuesOf('result')]}
		/>
		<Checkbox
			label="Push the run branch when the node succeeds"
			bind:checked={() => node.push, (v) => (node.push = v)}
		/>
	{:else if node.type === 'command'}
		<p class="ez-muted">
			Runs in the run's worktree: a mise task (preferred; reviewed in the repository, no quoting) or
			a shell string.
		</p>
		<div class="ez-cluster ez-cluster--end">
			<TextField
				label="Mise task"
				class="ez-control ez-mono"
				bind:value={node.task}
				issues={issuesOf('task')}
			/>
		</div>
		<ListField
			label="Arguments"
			help="One template per line, passed as one argument each."
			bind:value={node.args}
			issues={issuesOf('args')}
		/>
		<TextArea
			label="Shell"
			help={'Instead of a task: run with sh -c; every {{ … }} value is shell-quoted.'}
			rows={3}
			class="ez-control ez-mono"
			bind:value={node.run}
			issues={issuesOf('run')}
		/>
	{:else if node.type === 'action'}
		<Select
			label="Action"
			options={actionOptions}
			bind:value={() => node.action, (v) => (node.action = v as NodeDoc['action'])}
			issues={issuesOf('action')}
		/>
		{#if node.action === 'open_pull_request'}
			<TextField label="Title" bind:value={node.title} issues={issuesOf('title')} />
		{/if}
		{#if node.action === 'comment' || node.action === 'open_pull_request'}
			<TextArea
				label="Body"
				help="A template."
				rows={4}
				bind:value={node.body}
				issues={issuesOf('body')}
			/>
		{/if}
		{#if node.action === 'add_labels' || node.action === 'remove_labels'}
			<ListField label="Labels" bind:value={node.labels} issues={issuesOf('labels')} />
		{/if}
		{#if node.action === 'open_pull_request'}
			<Checkbox
				label="Open as a draft"
				bind:checked={() => node.draft === true, (v) => (node.draft = v ? true : null)}
			/>
		{/if}
	{:else if node.type === 'wait'}
		<ListField
			label="Events"
			help="One event or event.action per line; only events about the run's subject count."
			bind:value={node.on}
			issues={issuesOf('on')}
		/>
		<TextField label="If" class="ez-control ez-mono" bind:value={node.if} issues={issuesOf('if')} />
		<TextField
			label="Timeout"
			help="e.g. 7d; empty means the run's timeout."
			bind:value={node.timeout}
			issues={issuesOf('timeout')}
		/>
	{:else}
		<Select
			label="Status"
			options={statusOptions}
			bind:value={() => node.status, (v) => (node.status = v as NodeDoc['status'])}
		/>
		{#if node.status !== 'done'}
			<TextArea
				label="Reason"
				help="A template shown as the run's reason; empty records which node the run ended at."
				rows={2}
				bind:value={node.reason}
				issues={issuesOf('reason')}
			/>
		{/if}
	{/if}

	{#if node.type !== 'end'}
		<EdgesField
			bind:edges={node.next}
			{targets}
			outputs={NODE_OUTPUTS[node.type]}
			issues={issuesOf('next')}
		/>
		<TextField
			label="Max visits"
			help="How often a run may enter this node (loops); empty means 5."
			type="number"
			min="1"
			bind:value={visits.get, visits.set}
			issues={issuesOf('max_visits')}
		/>
	{/if}
</div>
