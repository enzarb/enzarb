<script lang="ts">
	import Button from '../form/Button.svelte';
	import Select from '../form/Select.svelte';
	import Badge from '../ui/Badge.svelte';
	import { issueModeIndex, type ModeDto, type ModeIssue } from '#lib/modes-types.ts';

	/** The modes in file order, the selected one marked; add blank or from an `ez init` template. */
	let {
		modes,
		defaultMode,
		selected = $bindable(0),
		issues,
		templates,
		add
	}: {
		modes: ModeDto[];
		defaultMode: string;
		selected?: number;
		issues: readonly ModeIssue[];
		templates: { set: string; mode: ModeDto }[];
		add: (from: ModeDto | null) => void;
	} = $props();

	const errorsIn = (i: number) =>
		issues.filter((x) => x.severity === 'error' && issueModeIndex(x.path) === i).length;
	const templateOptions = $derived(
		templates.map((t, i) => ({
			value: String(i),
			label: `${t.mode.id} (${t.set})`,
			description: t.mode.description
		}))
	);
	let template = $state('');
</script>

<div class="ez-stack">
	<p class="ez-muted">Modes are agent presets: a prompt, tools and a model.</p>
	<ul class="ez-modes-list" aria-label="Modes">
		{#each modes as m, i (i)}
			<li>
				<button
					type="button"
					class="ez-modes-list__item"
					aria-current={i === selected ? 'true' : undefined}
					onclick={() => (selected = i)}
				>
					<span class="ez-mono">{m.id || '(no id)'}</span>
					{#if m.id === defaultMode}<Badge tone="accent">default</Badge>{/if}
					{#if errorsIn(i)}<Badge tone="danger"
							>{errorsIn(i)} {errorsIn(i) === 1 ? 'error' : 'errors'}</Badge
						>{/if}
				</button>
			</li>
		{/each}
	</ul>
	{#if !modes.length}<p class="ez-muted">No modes yet.</p>{/if}
	<div><Button onclick={() => add(null)}>Add blank mode</Button></div>
	{#if templates.length}
		<div class="ez-cluster ez-cluster--end">
			<Select
				label="From a template"
				options={templateOptions}
				bind:value={template}
				placeholder="Choose a mode…"
			/>
			<Button
				disabled={!template}
				onclick={() => {
					add(templates[Number(template)].mode);
					template = '';
				}}>Add</Button
			>
		</div>
	{/if}
</div>
