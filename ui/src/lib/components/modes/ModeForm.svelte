<script lang="ts">
	import Button from '../form/Button.svelte';
	import Checkbox from '../form/Checkbox.svelte';
	import FieldSet from '../form/FieldSet.svelte';
	import TextArea from '../form/TextArea.svelte';
	import TextField from '../form/TextField.svelte';
	import HooksField from './HooksField.svelte';
	import ListField from './ListField.svelte';
	import PermissionsField from './PermissionsField.svelte';
	import ToolPicker from './ToolPicker.svelte';
	import { issuesAt, type ModeDto, type ModeIssue, type ModesCatalog } from '#lib/modes-types.ts';

	/**
	 * One mode's settings. `index` locates its issues (`modes[index]...`); the id is renamed through
	 * `rename` so references elsewhere in the document follow.
	 */
	let {
		mode = $bindable(),
		index,
		others,
		isDefault,
		catalog,
		issues,
		rename,
		remove,
		makeDefault
	}: {
		mode: ModeDto;
		index: number;
		others: string[];
		isDefault: boolean;
		catalog: ModesCatalog;
		issues: readonly ModeIssue[];
		rename: (id: string) => void;
		remove: () => void;
		makeDefault: () => void;
	} = $props();

	const at = $derived(`modes[${index}]`);
	const issuesOf = (field: string) => issuesAt(issues, `${at}.${field}`);
	const opt = (v: string) => v.trim() || null;

	function toggle(list: 'next' | 'autoAdvanceTo', id: string, on: boolean) {
		const keep = (o: string) => (o === id ? on : mode[list].includes(o));
		mode[list] = others.filter(keep);
		// Auto-advance targets must also be listed in `next`.
		if (list === 'autoAdvanceTo' && on && !mode.next.includes(id)) mode.next = [...mode.next, id];
		if (list === 'next' && !on) mode.autoAdvanceTo = mode.autoAdvanceTo.filter((o) => o !== id);
	}
</script>

<div class="ez-stack">
	<div class="ez-cluster ez-cluster--end">
		<TextField
			label="Id"
			help="Used in next, in workflows' agent nodes and as the section name in the file."
			class="ez-control ez-mono"
			bind:value={() => mode.id, (v) => rename(v.trim())}
			issues={issuesOf('id')}
		/>
	</div>
	<div class="ez-cluster">
		{#if isDefault}
			<span class="ez-muted">Default mode: new chats start here.</span>
		{:else}
			<Button size="sm" onclick={makeDefault}>Make default</Button>
		{/if}
		<Button size="sm" variant="danger" onclick={remove}>Remove mode</Button>
	</div>

	<TextField
		label="Description"
		help="One line shown in the mode picker."
		bind:value={mode.description}
		issues={issuesOf('description')}
	/>
	<TextArea
		label="Prompt"
		help="Appended to ez's system prompt while this mode is active."
		rows={10}
		bind:value={mode.prompt}
		issues={issuesOf('prompt')}
	/>

	<ToolPicker bind:tools={mode.tools} {catalog} issues={issuesOf('tools')} />
	<TextField
		label="Model"
		help="Empty uses the profile's model."
		bind:value={() => mode.model ?? '', (v) => (mode.model = opt(v))}
		issues={issuesOf('model')}
	/>

	<details class="ez-advanced">
		<summary>Advanced</summary>
		<div class="ez-stack">
			<TextField
				label="Display name"
				bind:value={() => mode.name ?? '', (v) => (mode.name = opt(v))}
				issues={issuesOf('name')}
			/>
			<TextField
				label="Profile"
				help="Backend profile; empty uses the default."
				bind:value={() => mode.profile ?? '', (v) => (mode.profile = opt(v))}
				issues={issuesOf('profile')}
			/>
			<ListField
				label="Mise tasks"
				help="One task name per line; the agent can run these as tools."
				bind:value={mode.miseTasks}
				issues={issuesOf('miseTasks')}
			/>
			<ListField
				label="MCP tools"
				help="One tool name or glob per line, e.g. mcp__github__*."
				bind:value={mode.mcpTools}
				issues={issuesOf('mcpTools')}
			/>

			{#if others.length}
				<FieldSet legend="Next modes">
					{#each others as id (id)}
						<div class="ez-cluster">
							<Checkbox
								label={id}
								bind:checked={() => mode.next.includes(id), (on) => toggle('next', id, on)}
							/>
							<Checkbox
								label="advance automatically"
								bind:checked={
									() => mode.autoAdvanceTo.includes(id), (on) => toggle('autoAdvanceTo', id, on)
								}
							/>
						</div>
					{/each}
					{#each [...issuesOf('next'), ...issuesOf('autoAdvanceTo')] as msg, n (n)}<p
							class="ez-field__error"
						>
							{msg}
						</p>{/each}
				</FieldSet>
			{/if}

			<PermissionsField
				bind:permissions={mode.permissions}
				values={catalog.permissionValues}
				issues={issuesOf('permissions')}
			/>
			<HooksField bind:hooks={mode.hooks} {catalog} {issues} base="{at}.hooks" />
		</div>
	</details>
</div>
