<script lang="ts">
	import Button from '#lib/components/form/Button.svelte';
	import { BUILTIN_PROMPTS } from '#lib/workflow/builtin-prompts.ts';
	import {
		NAMED_PROMPTS,
		PROMPT_DIR,
		isPromptPath,
		promptKind,
		type Prompt
	} from '#lib/workflow/definition.ts';

	/**
	 * An agent node's prompt: a named built-in template (read-only, with "copy into repo as file"),
	 * inline text, or `{file: .enzarb/prompts/<name>.md}` whose contents are edited here too.
	 */
	let {
		prompt = $bindable(),
		fileText,
		setFileText
	}: {
		prompt: Prompt | undefined;
		/** Current text of a prompt file; `undefined` while loading, `null` when it does not exist yet. */
		fileText: (path: string) => string | null | undefined;
		setFileText: (path: string, text: string) => void;
	} = $props();

	const uid = $props.id();
	const kind = $derived(promptKind(prompt));
	const named = $derived(
		kind === 'named' ? ((prompt as string).trim() as (typeof NAMED_PROMPTS)[number]) : null
	);
	const path = $derived(typeof prompt === 'object' ? prompt.file : '');
	const text = $derived(path ? fileText(path) : undefined);

	function setKind(k: string) {
		if (k === 'named') prompt = 'implement';
		else if (k === 'inline') prompt = '';
		else prompt = { file: `${PROMPT_DIR}prompt.md` };
	}

	function copyIntoRepo(name: (typeof NAMED_PROMPTS)[number]) {
		const file = `${PROMPT_DIR}${name}.md`;
		if (fileText(file) == null) setFileText(file, BUILTIN_PROMPTS[name]);
		prompt = { file };
	}
</script>

<div class="wf-prompt">
	<label class="ez-field__label" for="{uid}-kind">Prompt</label>
	<select
		id="{uid}-kind"
		class="ez-control"
		value={kind}
		onchange={(e) => setKind(e.currentTarget.value)}
	>
		<option value="named">Built-in template</option>
		<option value="inline">Inline text</option>
		<option value="file">File in the repository</option>
	</select>

	{#if named}
		<label class="ez-field__label" for="{uid}-named">Template</label>
		<select id="{uid}-named" class="ez-control" bind:value={prompt}>
			{#each NAMED_PROMPTS as n (n)}<option value={n}>{n}</option>{/each}
		</select>
		<pre class="wf-prompt__builtin" aria-label="Built-in template text">{BUILTIN_PROMPTS[
				named
			]}</pre>
		<div><Button size="sm" onclick={() => copyIntoRepo(named)}>Copy into repo as file</Button></div>
	{:else if kind === 'inline'}
		<label class="ez-field__label" for="{uid}-inline">Prompt text</label>
		<textarea
			id="{uid}-inline"
			class="ez-control wf-prompt__text"
			rows="8"
			value={prompt as string}
			oninput={(e) => (prompt = e.currentTarget.value)}></textarea>
	{:else}
		<label class="ez-field__label" for="{uid}-path">File</label>
		<input
			id="{uid}-path"
			class="ez-control"
			value={path}
			aria-invalid={isPromptPath(path) ? undefined : 'true'}
			onchange={(e) => (prompt = { file: e.currentTarget.value.trim() })}
		/>
		{#if !isPromptPath(path)}
			<p class="wf-prompt__problem">Must be {PROMPT_DIR}&lt;name&gt;.md</p>
		{:else if text === undefined}
			<p class="wf-prompt__muted">Loading {path}…</p>
		{:else}
			<label class="ez-field__label" for="{uid}-file">Contents of {path}</label>
			{#if text === null}<p class="wf-prompt__muted">
					New file: it is created with the change.
				</p>{/if}
			<textarea
				id="{uid}-file"
				class="ez-control wf-prompt__text"
				rows="12"
				value={text ?? ''}
				oninput={(e) => setFileText(path, e.currentTarget.value)}></textarea>
		{/if}
	{/if}
</div>

<style>
	.wf-prompt {
		display: grid;
		gap: var(--ez-space-2);
	}
	.wf-prompt__text {
		font-family: monospace;
		min-height: 8rem;
		resize: vertical;
	}
	.wf-prompt__builtin {
		max-height: 16rem;
		overflow: auto;
		white-space: pre-wrap;
		padding: var(--ez-space-2);
		background: var(--ez-surface-sunken);
		border-radius: var(--ez-radius-sm);
		font-size: 0.75rem;
	}
	.wf-prompt__problem {
		color: var(--ez-danger-fg);
		margin: 0;
	}
	.wf-prompt__muted {
		color: var(--ez-text-muted);
		margin: 0;
	}
</style>
