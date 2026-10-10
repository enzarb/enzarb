<script lang="ts">
	// Test-only: every control inside one plain form, showing what the browser would POST.
	import Button from '../Button.svelte';
	import Checkbox from '../Checkbox.svelte';
	import Combobox from '../Combobox.svelte';
	import RadioGroup from '../RadioGroup.svelte';
	import Select from '../Select.svelte';
	import Switch from '../Switch.svelte';
	import TextArea from '../TextArea.svelte';
	import TextField from '../TextField.svelte';

	/** With a form id, names carry SvelteKit's remote-form encoding (`b:x/<id>`, `x[]/<id>`). */
	let { formId }: { formId?: string } = $props();
	const n = (key: string) =>
		formId ? `${key}/${formId}` : key.replace(/^b:/, '').replace('[]', '');

	let posted = $state('');
	let events = $state<string[]>([]);
	function submit(e: SubmitEvent) {
		e.preventDefault();
		const data = new FormData(e.currentTarget as HTMLFormElement);
		posted = JSON.stringify([...data.entries()]);
	}
	const fruit = [
		{ value: 'apple', label: 'Apple' },
		{ value: 'pear', label: 'Pear' }
	];
</script>

<form onsubmit={submit} oninput={(e) => events.push((e.target as HTMLInputElement).name)}>
	<TextField label="Name" name={n('name')} value="abc" issues={[{ message: 'too short' }]} />
	<TextArea label="Notes" name={n('notes')} value="hi" />
	<Select label="Fruit" name={n('fruit')} options={fruit} value="pear" />
	<Combobox label="Repos" name={n('repos[]')} options={fruit} value={['apple', 'pear']} />
	<Checkbox label="Issues" name={n('b:issues')} checked />
	<Checkbox label="Mentions" name={n('b:mentions')} />
	<Switch label="Live" name={n('b:live')} checked />
	<RadioGroup label="Size" name={n('size')} options={fruit} value="apple" />
	<Button type="submit">Send</Button>
	<Button type="reset">Reset</Button>
</form>
<output data-testid="posted">{posted}</output>
<output data-testid="events">{JSON.stringify(events)}</output>
