<script lang="ts">
	import type { QuestionAnswer, QuestionStatus, QuestionView } from '#lib/acp/types.ts';

	/**
	 * ez's `AskUserQuestion`: one group per question (radio for single choice, checkboxes for
	 * multiple) plus an optional free-text "Other". Nothing is preselected. `epoch` is the
	 * connection epoch the item was rendered in.
	 */
	let {
		id,
		questions,
		status,
		epoch,
		onAnswer
	}: {
		id: string;
		questions: QuestionView[];
		status: QuestionStatus;
		epoch: number;
		onAnswer: (id: string, answers: QuestionAnswer[] | null, epoch: number) => void;
	} = $props();

	const uid = $props.id();
	let selected = $state<string[][]>([]);
	let other = $state<string[]>([]);

	const answered = (i: number) =>
		(selected[i]?.length ?? 0) > 0 || (other[i] ?? '').trim().length > 0;
	const complete = $derived(questions.every((_, i) => answered(i)));

	function toggle(i: number, label: string, multi: boolean, on: boolean) {
		const cur = selected[i] ?? [];
		const next = multi
			? on
				? [...cur.filter((l) => l !== label), label]
				: cur.filter((l) => l !== label)
			: on
				? [label]
				: [];
		const all = [...selected];
		all[i] = next;
		selected = all;
	}

	function submit(e: SubmitEvent) {
		e.preventDefault();
		if (!complete) return;
		onAnswer(
			id,
			questions.map((q, i) => ({
				question: q.question,
				// keep the agent's option order
				selected: q.options.map((o) => o.label).filter((l) => selected[i]?.includes(l)),
				other: (other[i] ?? '').trim() || null
			})),
			epoch
		);
	}

	const words: Record<Exclude<QuestionStatus, 'pending'>, string> = {
		answered: 'Answered',
		cancelled: 'Cancelled'
	};
</script>

<div class="ez-chat-perm ez-chat-question" data-status={status}>
	<p class="ez-chat-perm__title">
		The agent has {questions.length === 1 ? 'a question' : 'questions'}
	</p>
	{#if status === 'pending'}
		<form onsubmit={submit}>
			{#each questions as q, i (i)}
				<fieldset class="ez-chat-question__group">
					<legend>
						{#if q.header}<span class="ez-chat-question__header">{q.header}</span>{/if}
						{q.question}
					</legend>
					{#each q.options as o, j (o.label)}
						<label class="ez-chat-question__option">
							<input
								type={q.multiSelect ? 'checkbox' : 'radio'}
								name="{uid}-{i}"
								value={o.label}
								checked={selected[i]?.includes(o.label) ?? false}
								onchange={(e) => toggle(i, o.label, q.multiSelect, e.currentTarget.checked)}
								aria-describedby={o.description ? `${uid}-${i}-${j}-d` : undefined}
							/>
							<span>{o.label}</span>
							{#if o.description}
								<span class="ez-muted" id="{uid}-{i}-{j}-d">{o.description}</span>
							{/if}
						</label>
					{/each}
					<label class="ez-field">
						<span class="ez-field__label">Other</span>
						<input
							class="ez-control"
							type="text"
							maxlength="4096"
							value={other[i] ?? ''}
							oninput={(e) => {
								const all = [...other];
								all[i] = e.currentTarget.value;
								other = all;
							}}
						/>
					</label>
				</fieldset>
			{/each}
			<div class="ez-chat-perm__actions" role="group" aria-label="Respond to the question">
				<button type="submit" class="ez-btn ez-btn--secondary ez-btn--sm" disabled={!complete}>
					Answer
				</button>
				<button
					type="button"
					class="ez-btn ez-btn--secondary ez-btn--sm"
					onclick={() => onAnswer(id, null, epoch)}
				>
					Skip
				</button>
			</div>
		</form>
	{:else}
		<ul class="ez-chat-question__summary">
			{#each questions as q, i (i)}<li>{q.question}</li>{/each}
		</ul>
		<p class="ez-chat-perm__state">{words[status]}</p>
	{/if}
</div>
