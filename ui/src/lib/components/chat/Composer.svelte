<script lang="ts">
	import { MAX_PROMPT_BYTES } from '#lib/acp/types.ts';

	/** Prompt box. Enter sends, Shift+Enter adds a line; Stop replaces nothing, it sits beside Send. */
	let {
		busy = false,
		disabled = false,
		onSend,
		onStop
	}: {
		busy?: boolean;
		disabled?: boolean;
		onSend: (text: string) => void;
		onStop: () => void;
	} = $props();

	let text = $state('');
	const bytes = $derived(new TextEncoder().encode(text).length);
	const tooLong = $derived(bytes > MAX_PROMPT_BYTES);
	const canSend = $derived(!disabled && !busy && !tooLong && text.trim().length > 0);

	function send() {
		if (!canSend) return;
		onSend(text);
		text = '';
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
			e.preventDefault();
			send();
		}
	}
</script>

<form
	class="ez-chat-composer"
	onsubmit={(e) => {
		e.preventDefault();
		send();
	}}
>
	<label class="ez-sr-only" for="chat-prompt">Message</label>
	<textarea
		id="chat-prompt"
		class="ez-chat-composer__input"
		rows="3"
		bind:value={text}
		{onkeydown}
		{disabled}
		aria-invalid={tooLong}
		aria-describedby={tooLong ? 'chat-prompt-limit' : undefined}
		placeholder="Message the agent (Enter to send, Shift+Enter for a new line)"></textarea>
	{#if tooLong}
		<p id="chat-prompt-limit" class="ez-chat-composer__limit" role="alert">
			This message is too long: the limit is {MAX_PROMPT_BYTES / 1024} KiB.
		</p>
	{/if}
	<div class="ez-chat-composer__actions">
		{#if busy}
			<button type="button" class="ez-btn ez-btn--danger ez-btn--sm" onclick={onStop}>Stop</button>
		{/if}
		<button type="submit" class="ez-btn ez-btn--primary ez-btn--sm" disabled={!canSend}>Send</button
		>
	</div>
</form>
