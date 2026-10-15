<script lang="ts" module>
	import type { ChangedFileInput } from '#lib/workflow/propose.ts';

	export type ProposeFn = (
		files: ChangedFileInput[],
		workflow: string,
		step: (id: 'branch' | 'commit' | 'pr', message?: string) => void
	) => Promise<{ url: string }>;
</script>

<script lang="ts">
	import Button from '#lib/components/form/Button.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import {
		applyToDocument,
		check,
		NODE_TYPES,
		promptKind,
		validate,
		WORKFLOW_DIR,
		type Definition,
		type NodeType
	} from '#lib/workflow/definition.ts';
	import { connect, deleteNode, freeId, newNode, renameNode, retype } from '#lib/workflow/edit.ts';
	import { parseDocument } from 'yaml';
	import NodePanel from './NodePanel.svelte';
	import ProposeChecklist, { type ProposeStep } from './ProposeChecklist.svelte';
	import WorkflowCanvas from './WorkflowCanvas.svelte';

	/**
	 * The workflow editor: graph or YAML view of one definition, a side panel for the selected node,
	 * live validation, and "propose change" (branch, commit, pull request; never the default branch).
	 */
	let {
		source,
		path,
		loadFile,
		propose
	}: {
		/** The definition as on the default branch (or the example for a new one). */
		source: string;
		/** Its path, or `null` for a new definition (named after `name`). */
		path: string | null;
		/** Reads a prompt file from the default branch; `null` when it does not exist. */
		loadFile: (path: string) => Promise<string | null>;
		propose: ProposeFn;
	} = $props();

	function initial() {
		const r = check(source);
		return { def: r.def ?? null, parse: r.parse ?? null, text: source };
	}
	// The editor starts from `source`; the page remounts it for another file.
	const start = initial();
	/** The model; `null` while the YAML does not parse. */
	let def = $state<Definition | null>(start.def);
	/** YAML text the serialization preserves comments and layout from. */
	let base = $state(start.text);
	let view = $state<'graph' | 'yaml'>(start.def ? 'graph' : 'yaml');
	let yamlText = $state(start.text);
	let parseError = $state<string | null>(start.parse);
	let selected = $state<string | null>(null);
	let renameError = $state<string | null>(null);
	/** Prompt files: current text (`null` = does not exist) and the default-branch text. */
	let files = $state<
		Record<string, { text: string | null; original: string | null; loaded: boolean }>
	>({});
	let steps = $state<ProposeStep[] | null>(null);
	let prUrl = $state<string | undefined>();
	let busy = $state(false);

	const problems = $derived(def ? validate(def) : []);
	const byNode = $derived.by(() => {
		const m: Record<string, string[]> = {};
		for (const p of problems) if (p.node) (m[p.node] ??= []).push(p.message);
		return new Map(Object.entries(m));
	});
	const yaml = $derived(
		def ? applyToDocument(parseDocument(base), $state.snapshot(def) as Definition, base) : yamlText
	);
	const ids = $derived(def?.nodes.map((n) => n.id) ?? []);
	const selectedIndex = $derived(
		def && selected ? def.nodes.findIndex((n) => n.id === selected) : -1
	);
	const defPath = $derived(
		path ?? `${WORKFLOW_DIR}${(def?.name || 'workflow').replace(/[^A-Za-z0-9_-]+/g, '-')}.yaml`
	);
	const promptPaths = $derived(
		(def?.nodes ?? []).flatMap((n) =>
			n.type === 'agent' && promptKind(n.prompt) === 'file'
				? [(n.prompt as { file: string }).file]
				: []
		)
	);
	const changed = $derived.by(() => {
		const out: ChangedFileInput[] = [];
		if (yaml !== source || path === null) out.push({ path: defPath, text: yaml });
		for (const p of promptPaths) {
			const f = files[p];
			if (f && f.text !== null && f.text !== f.original) out.push({ path: p, text: f.text });
		}
		return out;
	});
	const canPropose = $derived(
		!!def && !parseError && problems.length === 0 && changed.length > 0 && !busy
	);

	// Load prompt files the definition references (once each).
	$effect(() => {
		for (const p of promptPaths) {
			if (p in files || !/^\.enzarb\/prompts\/[^/]+\.md$/.test(p)) continue;
			files[p] = { text: null, original: null, loaded: false };
			const done = (t: string | null) => {
				// A file the user already started writing keeps their text.
				const cur = files[p];
				files[p] = { text: cur.text ?? t, original: t, loaded: true };
			};
			loadFile(p).then(done, () => done(null));
		}
	});

	function fileText(p: string): string | null | undefined {
		const f = files[p];
		if (f && !f.loaded && f.text === null) return undefined;
		return f?.text ?? null;
	}
	function setFileText(p: string, text: string) {
		const f = files[p];
		files[p] = { text, original: f?.original ?? null, loaded: f?.loaded ?? true };
	}

	function showYaml() {
		yamlText = yaml;
		view = 'yaml';
	}
	function editYaml(text: string) {
		yamlText = text;
		const r = check(text);
		parseError = r.parse ?? null;
		if (r.def) {
			def = r.def;
			base = text;
		}
	}

	function addNode(type: NodeType) {
		if (!def) return;
		const id = freeId(def, type === 'end' ? 'end' : type);
		def.nodes.push(newNode(id, type));
		if (!def.nodes.some((n) => n.id === def!.start)) def.start = id;
		selected = id;
	}

	function rename(to: string) {
		if (!def || !selected) return;
		if (to !== selected && def.nodes.some((n) => n.id === to)) {
			renameError = `A node \`${to}\` already exists.`;
			return;
		}
		renameError = null;
		renameNode(def, selected, to);
		selected = to;
	}

	async function submit() {
		if (!def) return;
		busy = true;
		prUrl = undefined;
		steps = [
			{ id: 'branch', status: 'active' },
			{ id: 'commit', status: 'pending' },
			{ id: 'pr', status: 'pending' }
		];
		const order = ['branch', 'commit', 'pr'] as const;
		let at = 0;
		try {
			const { url } = await propose($state.snapshot(changed), def.name, (id, message) => {
				const i = order.indexOf(id);
				steps = steps!.map((s, j) =>
					j < i
						? s
						: j === i
							? { ...s, status: 'done', message }
							: j === i + 1
								? { ...s, status: 'active' }
								: s
				);
				at = i + 1;
			});
			prUrl = url;
		} catch (e) {
			const msg = (e as { body?: { message?: string } }).body?.message ?? (e as Error).message;
			steps = steps!.map((s, j) => (j === at ? { ...s, status: 'failed', message: msg } : s));
		} finally {
			busy = false;
		}
	}
</script>

<div class="wf-editor">
	<div class="wf-editor__bar">
		<div class="wf-editor__views" role="group" aria-label="View">
			<Button
				size="sm"
				variant={view === 'graph' ? 'primary' : 'secondary'}
				aria-pressed={view === 'graph'}
				disabled={!def || !!parseError}
				onclick={() => (view = 'graph')}>Graph</Button
			>
			<Button
				size="sm"
				variant={view === 'yaml' ? 'primary' : 'secondary'}
				aria-pressed={view === 'yaml'}
				onclick={showYaml}>YAML</Button
			>
		</div>
		{#if view === 'graph' && def}
			<label class="wf-editor__add">
				<span>Add node</span>
				<select
					class="ez-control"
					value=""
					onchange={(e) => {
						addNode(e.currentTarget.value as NodeType);
						e.currentTarget.value = '';
					}}
				>
					<option value="" disabled>type…</option>
					{#each NODE_TYPES as t (t)}<option value={t}>{t}</option>{/each}
				</select>
			</label>
		{/if}
		<span class="wf-editor__path">{defPath}</span>
		<Button variant="primary" disabled={!canPropose} onclick={submit}>Propose change</Button>
	</div>

	{#if parseError}
		<Notice tone="error" title="The YAML does not parse">{parseError}</Notice>
	{/if}

	{#if view === 'yaml' || !def}
		<label class="ez-field__label" for="wf-yaml">Definition (YAML)</label>
		<textarea
			id="wf-yaml"
			class="ez-control wf-editor__yaml"
			rows="24"
			spellcheck="false"
			value={yamlText}
			oninput={(e) => editYaml(e.currentTarget.value)}></textarea>
	{:else}
		<div class="wf-editor__main">
			<WorkflowCanvas
				{def}
				problems={byNode}
				{selected}
				onselect={(id) => (selected = id)}
				onconnect={(from, to) => def && connect(def, from, to)}
			/>
			<aside class="wf-editor__side">
				{#if selectedIndex >= 0}
					{#if renameError}<Notice tone="error">{renameError}</Notice>{/if}
					<NodePanel
						bind:node={def.nodes[selectedIndex]}
						{ids}
						isStart={def.start === selected}
						problems={byNode.get(selected!) ?? []}
						onrename={rename}
						onretype={(t) =>
							def && (def.nodes[selectedIndex] = retype(def.nodes[selectedIndex], t))}
						onsetstart={() => def && selected && (def.start = selected)}
						ondelete={() => {
							if (def && selected) deleteNode(def, selected);
							selected = null;
						}}
						{fileText}
						{setFileText}
					/>
				{:else}
					<div class="wf-editor__meta">
						<div class="ez-field">
							<label class="ez-field__label" for="wf-name">Name</label>
							<input id="wf-name" class="ez-control" bind:value={def.name} />
						</div>
						<div class="ez-field">
							<label class="ez-field__label" for="wf-on">Triggers (comma separated)</label>
							<input
								id="wf-on"
								class="ez-control"
								value={def.on.join(', ')}
								onchange={(e) =>
									def &&
									(def.on = e.currentTarget.value
										.split(',')
										.map((s) => s.trim())
										.filter(Boolean))}
							/>
						</div>
						<p class="wf-editor__hint">
							Select a node to edit it. Drag from a node's bottom handle to another node to add an
							edge.
						</p>
					</div>
				{/if}
			</aside>
		</div>
	{/if}

	<section aria-label="Validation">
		{#if problems.length}
			<Notice tone="warning" title="{problems.length} problem{problems.length === 1 ? '' : 's'}">
				<ul class="wf-editor__problems">
					{#each problems as p, i (i)}
						<li>
							{#if p.node && ids.includes(p.node)}
								<button
									type="button"
									class="wf-editor__link"
									onclick={() => {
										selected = p.node!;
										view = 'graph';
									}}>{p.message}</button
								>
							{:else}{p.message}{/if}
						</li>
					{/each}
				</ul>
			</Notice>
		{:else if def && !parseError}
			<p class="wf-editor__ok">The definition is valid.</p>
		{/if}
	</section>

	{#if steps}<ProposeChecklist {steps} url={prUrl} />{/if}
</div>

<style>
	.wf-editor {
		display: grid;
		gap: var(--ez-space-3);
	}
	.wf-editor__bar {
		display: flex;
		flex-wrap: wrap;
		gap: var(--ez-space-3);
		align-items: center;
	}
	.wf-editor__views {
		display: flex;
		gap: var(--ez-space-1);
	}
	.wf-editor__add {
		display: flex;
		gap: var(--ez-space-2);
		align-items: center;
	}
	.wf-editor__path {
		margin-left: auto;
		font-family: monospace;
		color: var(--ez-text-muted);
	}
	.wf-editor__main {
		display: grid;
		grid-template-columns: minmax(0, 2fr) minmax(280px, 1fr);
		gap: var(--ez-space-3);
		min-height: 560px;
	}
	@media (max-width: 800px) {
		.wf-editor__main {
			grid-template-columns: 1fr;
		}
	}
	.wf-editor__side {
		max-height: 80vh;
		overflow: auto;
	}
	.wf-editor__meta {
		display: grid;
		gap: var(--ez-space-3);
	}
	.wf-editor__hint,
	.wf-editor__ok {
		color: var(--ez-text-muted);
		margin: 0;
	}
	.wf-editor__yaml {
		font-family: monospace;
		min-height: 24rem;
	}
	.wf-editor__problems {
		margin: 0;
		padding-left: var(--ez-space-4);
	}
	.wf-editor__link {
		border: 0;
		padding: 0;
		background: none;
		color: inherit;
		text-align: left;
		text-decoration: underline;
		cursor: pointer;
		font: inherit;
	}
</style>
