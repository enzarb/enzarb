<script lang="ts">
	import type { TextRole } from '#lib/acp/types.ts';

	/** One user, agent or thought message. The text is untrusted and only ever interpolated. */
	let { kind, text }: { kind: TextRole; text: string } = $props();

	const who: Record<TextRole, string> = { user: 'You', agent: 'Agent', thought: 'Thinking' };
</script>

<div class="ez-chat-msg ez-chat-msg--{kind}">
	<span class="ez-chat-msg__who">{who[kind]}</span>
	<p class="ez-chat-msg__text">{text}</p>
</div>
