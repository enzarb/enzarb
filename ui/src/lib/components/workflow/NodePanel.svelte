<script lang="ts">
	import Button from '#lib/components/form/Button.svelte';
	import {
		DEFAULT_MAX_VISITS,
		END_STATUSES,
		KNOWN_ACTIONS,
		KNOWN_EVENTS,
		NODE_TYPES,
		WAIT_FROM,
		type NodeType,
		type WorkflowNode
	} from '#lib/workflow/definition.ts';
	import EdgeEditor from './EdgeEditor.svelte';
	import PromptField from './PromptField.svelte';

	/**
	 * Side panel for the selected node. Edits the node in place (it is part of the editor's state);
	 * id renames, type changes, start and delete go through callbacks because they touch other nodes.
	 */
	let {
		node = $bindable(),
		ids,
		isStart,
		problems,
		onrename,
		onretype,
		onsetstart,
		ondelete,
		fileText,
		setFileText
	}: {
		node: WorkflowNode;
		ids: string[];
		isStart: boolean;
		problems: string[];
		onrename: (to: string) => void;
		onretype: (type: NodeType) => void;
		onsetstart: () => void;
		ondelete: () => void;
		fileText: (path: string) => string | null | undefined;
		setFileText: (path: string, text: string) => void;
	} = $props();

	const uid = $props.id();
	const others = $derived(ids.filter((i) => i !== node.id));
	const sandbox = $derived(node.type === 'command' || node.type === 'agent');

	function toggleEvent(e: string, on: boolean) {
		const cur = node.events ?? [];
		node.events = on ? [...cur, e] : cur.filter((x) => x !== e);
	}
</script>

<section class="wf-panel" aria-label="Node {node.id}">
	<div class="wf-panel__row">
		<div class="ez-field">
			<label class="ez-field__label" for="{uid}-id">Id</label>
			<input
				id="{uid}-id"
				class="ez-control"
				value={node.id}
				onchange={(e) => onrename(e.currentTarget.value.trim())}
			/>
		</div>
		<div class="ez-field">
			<label class="ez-field__label" for="{uid}-type">Type</label>
			<select
				id="{uid}-type"
				class="ez-control"
				value={node.type}
				onchange={(e) => onretype(e.currentTarget.value as NodeType)}
			>
				{#each NODE_TYPES as t (t)}<option value={t}>{t}</option>{/each}
			</select>
		</div>
	</div>

	{#if problems.length}
		<ul class="wf-panel__problems" aria-label="Problems of this node">
			{#each problems as p (p)}<li>{p}</li>{/each}
		</ul>
	{/if}

	{#if node.type === 'command'}
		<div class="ez-field">
			<label class="ez-field__label" for="{uid}-run">Run</label>
			<textarea id="{uid}-run" class="ez-control wf-panel__code" rows="3" bind:value={node.run}
			></textarea>
		</div>
	{:else if node.type === 'agent'}
		<PromptField bind:prompt={node.prompt} {fileText} {setFileText} />
	{:else if node.type === 'wait'}
		<fieldset class="wf-panel__set">
			<legend>Events</legend>
			{#each KNOWN_EVENTS as e (e)}
				<label class="wf-panel__check">
					<input
						type="checkbox"
						checked={(node.events ?? []).includes(e)}
						onchange={(ev) => toggleEvent(e, ev.currentTarget.checked)}
					/>
					{e}
				</label>
			{/each}
		</fieldset>
		<div class="ez-field">
			<label class="ez-field__label" for="{uid}-from">Comments from</label>
			<select
				id="{uid}-from"
				class="ez-control"
				value={node.from ?? 'writers'}
				onchange={(e) => (node.from = e.currentTarget.value as (typeof WAIT_FROM)[number])}
			>
				{#each WAIT_FROM as f (f)}<option value={f}>{f}</option>{/each}
			</select>
		</div>
	{:else if node.type === 'action'}
		<div class="ez-field">
			<label class="ez-field__label" for="{uid}-action">Action</label>
			<select id="{uid}-action" class="ez-control" bind:value={node.action}>
				{#each KNOWN_ACTIONS as a (a)}<option value={a}>{a}</option>{/each}
			</select>
		</div>
		{#if node.action === 'post_comment'}
			<div class="ez-field">
				<label class="ez-field__label" for="{uid}-body">Body (empty: the agent's body)</label>
				<textarea
					id="{uid}-body"
					class="ez-control"
					rows="3"
					value={node.body ?? ''}
					oninput={(e) => (node.body = e.currentTarget.value || undefined)}></textarea>
			</div>
		{/if}
	{:else}
		<div class="ez-field">
			<label class="ez-field__label" for="{uid}-status">Status</label>
			<select id="{uid}-status" class="ez-control" bind:value={node.status}>
				{#each END_STATUSES as s (s)}<option value={s}>{s}</option>{/each}
			</select>
		</div>
	{/if}

	{#if node.type !== 'end'}
		{#if sandbox}
			<label class="wf-panel__check">
				<input
					type="checkbox"
					bind:checked={() => !!node.optional, (v) => (node.optional = v || undefined)}
				/>
				Optional (a failure follows the edges)
			</label>
		{/if}
		<EdgeEditor bind:next={node.next} targets={ids} />
		<div class="wf-panel__row">
			<div class="ez-field">
				<label class="ez-field__label" for="{uid}-visits">Max visits</label>
				<input
					id="{uid}-visits"
					class="ez-control"
					type="number"
					min="1"
					placeholder={String(DEFAULT_MAX_VISITS)}
					value={node.max_visits ?? ''}
					oninput={(e) => {
						const v = e.currentTarget.value;
						node.max_visits = v === '' ? undefined : Math.max(0, Math.trunc(Number(v)));
					}}
				/>
			</div>
			<div class="ez-field">
				<label class="ez-field__label" for="{uid}-limit">On limit</label>
				<select
					id="{uid}-limit"
					class="ez-control"
					value={node.on_limit ?? ''}
					onchange={(e) => (node.on_limit = e.currentTarget.value || undefined)}
				>
					<option value="">Fail the graph</option>
					{#each others as t (t)}<option value={t}>{t}</option>{/each}
				</select>
			</div>
		</div>
	{/if}

	<div class="wf-panel__actions">
		<Button size="sm" disabled={isStart} onclick={onsetstart}
			>{isStart ? 'Start node' : 'Set as start'}</Button
		>
		<Button size="sm" variant="danger" onclick={ondelete}>Delete node</Button>
	</div>
</section>

<style>
	.wf-panel {
		display: grid;
		gap: var(--ez-space-3);
		align-content: start;
	}
	.wf-panel__row {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--ez-space-2);
	}
	.wf-panel__code {
		font-family: monospace;
	}
	.wf-panel__set {
		display: flex;
		flex-wrap: wrap;
		gap: var(--ez-space-2);
		border: 1px solid var(--ez-border);
		border-radius: var(--ez-radius-sm);
	}
	.wf-panel__check {
		display: inline-flex;
		gap: var(--ez-space-1);
		align-items: center;
	}
	.wf-panel__problems {
		margin: 0;
		padding: var(--ez-space-2) var(--ez-space-2) var(--ez-space-2) var(--ez-space-5);
		color: var(--ez-danger-fg);
		background: var(--ez-danger-bg);
		border: 1px solid var(--ez-danger-border);
		border-radius: var(--ez-radius-sm);
		font-size: 0.85rem;
	}
	.wf-panel__actions {
		display: flex;
		gap: var(--ez-space-2);
	}
</style>
