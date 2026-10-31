<script lang="ts">
	import Button from '../form/Button.svelte';

	/**
	 * The chat bar pinned under a task's detail. A stub for now: a later change mounts the real
	 * `Chat` in the `chat` snippet of TaskDetail, in this same fixed-height slot.
	 */
</script>

<div class="ez-tasks__chatbar" data-testid="task-chat-bar">
	<textarea
		class="ez-tasks__chatinput"
		rows="2"
		disabled
		aria-label="Message the agent about this task"
		placeholder="Chat with the agent about this task: coming next"></textarea>
	<div class="ez-tasks__chatactions">
		<Button disabled>Send</Button>
	</div>
</div>
