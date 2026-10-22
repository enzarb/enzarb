<script lang="ts">
	import Button from '../form/Button.svelte';
	import Select from '../form/Select.svelte';
	import TextField from '../form/TextField.svelte';
	import ListField from '../modes/ListField.svelte';
	import { diagnosticsAt, type Diagnostic, type WorkflowDoc } from '#lib/workflows-types.ts';

	/** A workflow's trigger and limits; its nodes are edited from the graph. */
	let {
		workflow = $bindable(),
		diagnostics,
		rename,
		remove
	}: {
		workflow: WorkflowDoc;
		diagnostics: readonly Diagnostic[];
		rename: (id: string) => void;
		remove: () => void;
	} = $props();

	const at = $derived(`workflows.${workflow.id}`);
	const issuesOf = (field: string) => diagnosticsAt(diagnostics, `${at}.${field}`);
	const startOptions = $derived(workflow.nodes.map((n) => ({ value: n.id, label: n.id })));

	const maxRuns = {
		get: () => (workflow.max_runs === null ? '' : String(workflow.max_runs)),
		set: (v: string) => {
			const n = Number.parseInt(v, 10);
			workflow.max_runs = Number.isFinite(n) && n >= 0 ? n : null;
		}
	};
</script>

<div class="ez-stack">
	<TextField
		label="Workflow id"
		class="ez-control ez-mono"
		bind:value={() => workflow.id, (v) => rename(v.trim())}
		issues={diagnostics.filter((d) => d.path === at).map((d) => d.message)}
	/>
	<TextField label="Description" bind:value={workflow.description} />
	<ListField
		label="On"
		help="One event or event.action per line (issues.labeled, pull_request.opened), as logged in Events."
		bind:value={workflow.on}
		issues={issuesOf('on')}
	/>
	<TextField
		label="If"
		help="Optional filter over event and subject, e.g. event.label.name == 'ez'."
		class="ez-control ez-mono"
		bind:value={workflow.if}
		issues={issuesOf('if')}
	/>
	<Select
		label="Start node"
		options={startOptions}
		bind:value={workflow.start}
		issues={issuesOf('start')}
	/>
	<div class="ez-cluster ez-cluster--end">
		<TextField
			label="Timeout"
			help="e.g. 14d (the default)."
			bind:value={workflow.timeout}
			issues={issuesOf('timeout')}
		/>
		<TextField
			label="Max runs"
			help="Live runs at once; empty means 3."
			type="number"
			min="1"
			bind:value={maxRuns.get, maxRuns.set}
			issues={issuesOf('max_runs')}
		/>
	</div>
	<div><Button size="sm" variant="danger" onclick={remove}>Remove workflow</Button></div>
</div>
