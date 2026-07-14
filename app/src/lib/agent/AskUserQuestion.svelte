<script lang="ts">
	import type { QuestionPayload } from './types';

	let {
		message,
		questions,
		disabled = false,
		onRespond
	}: {
		message: string;
		questions: QuestionPayload[];
		disabled?: boolean;
		onRespond: (answers: Record<string, string | string[]> | null) => void;
	} = $props();

	// field_key -> selected option value(s); custom_field_key -> typed text.
	let values: Record<string, string | string[]> = $state({});

	function selectSingle(fieldKey: string, value: string) {
		values[fieldKey] = value;
	}

	function toggleMulti(fieldKey: string, value: string) {
		const current = Array.isArray(values[fieldKey]) ? (values[fieldKey] as string[]) : [];
		values[fieldKey] = current.includes(value)
			? current.filter((v) => v !== value)
			: [...current, value];
	}

	function setCustom(fieldKey: string, text: string) {
		if (text) values[fieldKey] = text;
		else delete values[fieldKey];
	}

	function submit() {
		const answers: Record<string, string | string[]> = {};
		for (const [key, value] of Object.entries(values)) {
			if (Array.isArray(value) ? value.length : value) answers[key] = value;
		}
		onRespond(Object.keys(answers).length ? answers : null);
	}

	function skip() {
		onRespond(null);
	}
</script>

<div class="ask-prompt">
	<div class="ask-message">{message}</div>
	{#each questions as q (q.field_key)}
		<div class="ask-question">
			{#if q.header}<div class="ask-header">{q.header}</div>{/if}
			{#if q.question}<div class="ask-text">{q.question}</div>{/if}
			<div class="ask-options">
				{#each q.options as opt (opt.value)}
					{#if q.multi_select}
						<label class="ask-option">
							<input
								type="checkbox"
								{disabled}
								checked={Array.isArray(values[q.field_key]) && (values[q.field_key] as string[]).includes(opt.value)}
								onchange={() => toggleMulti(q.field_key, opt.value)}
							/>
							{opt.label}
						</label>
					{:else}
						<label class="ask-option">
							<input
								type="radio"
								name={q.field_key}
								{disabled}
								checked={values[q.field_key] === opt.value}
								onchange={() => selectSingle(q.field_key, opt.value)}
							/>
							{opt.label}
						</label>
					{/if}
				{/each}
			</div>
			{#if q.custom_field_key}
				<input
					class="ask-custom"
					type="text"
					placeholder="Other (type your own answer)…"
					{disabled}
					value={(values[q.custom_field_key] as string) ?? ''}
					oninput={(e) => setCustom(q.custom_field_key!, e.currentTarget.value)}
				/>
			{/if}
		</div>
	{/each}
	<div class="ask-actions">
		<button class="ask-btn skip" {disabled} onclick={skip}>Skip</button>
		<button class="ask-btn submit" {disabled} onclick={submit}>Submit</button>
	</div>
	{#if disabled}
		<div class="ask-hint">Reconnecting — your response will be available once the connection is back.</div>
	{/if}
</div>

<style>
	.ask-prompt { border: 1px solid #4f8ef7; border-radius: 6px; padding: 0.6rem 0.8rem; background: color-mix(in srgb, #4f8ef7 8%, transparent); font-size: 12px; display: flex; flex-direction: column; gap: 0.6rem; }
	.ask-message { font-weight: 600; }
	.ask-question { display: flex; flex-direction: column; gap: 0.3rem; }
	.ask-header { font-weight: 600; }
	.ask-text { color: var(--color-text-muted); }
	.ask-options { display: flex; flex-direction: column; gap: 0.2rem; }
	.ask-option { display: flex; align-items: center; gap: 0.4rem; cursor: pointer; }
	.ask-custom { font-size: 12px; padding: 0.3rem 0.5rem; border-radius: 4px; border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-text); }
	.ask-actions { display: flex; gap: 0.4rem; }
	.ask-btn { font-size: 12px; padding: 0.3rem 0.7rem; border-radius: 4px; border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-text); cursor: pointer; }
	.ask-btn.submit { border-color: #4f8ef7; color: #4f8ef7; }
	.ask-btn.submit:hover { background: color-mix(in srgb, #4f8ef7 15%, transparent); }
	.ask-btn:disabled { opacity: 0.5; cursor: not-allowed; }
	.ask-hint { font-size: 11px; color: var(--color-text-muted); }
</style>
