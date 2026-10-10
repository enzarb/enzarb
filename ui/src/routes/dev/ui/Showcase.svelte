<script lang="ts">
	// Dev-only gallery: every shared component in every state. Rendered twice (light and dark).
	import ConditionChecklist from '#lib/components/ConditionChecklist.svelte';
	import Button from '#lib/components/form/Button.svelte';
	import Checkbox from '#lib/components/form/Checkbox.svelte';
	import Combobox from '#lib/components/form/Combobox.svelte';
	import FieldSet from '#lib/components/form/FieldSet.svelte';
	import RadioGroup from '#lib/components/form/RadioGroup.svelte';
	import Select from '#lib/components/form/Select.svelte';
	import Switch from '#lib/components/form/Switch.svelte';
	import TextArea from '#lib/components/form/TextArea.svelte';
	import TextField from '#lib/components/form/TextField.svelte';
	import StatusBadge from '#lib/components/StatusBadge.svelte';
	import WorkStateChip from '#lib/components/WorkStateChip.svelte';
	import WorkTimeline from '#lib/components/WorkTimeline.svelte';
	import Avatar from '#lib/components/ui/Avatar.svelte';
	import Badge from '#lib/components/ui/Badge.svelte';
	import Card from '#lib/components/ui/Card.svelte';
	import ConfirmDialog from '#lib/components/ui/ConfirmDialog.svelte';
	import DataTable, { type Column } from '#lib/components/ui/DataTable.svelte';
	import Dialog from '#lib/components/ui/Dialog.svelte';
	import DropdownMenu from '#lib/components/ui/DropdownMenu.svelte';
	import EmptyState from '#lib/components/ui/EmptyState.svelte';
	import Icon from '#lib/components/ui/Icon.svelte';
	import KeyValueList from '#lib/components/ui/KeyValueList.svelte';
	import LinkButton from '#lib/components/ui/LinkButton.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import StepList from '#lib/components/ui/StepList.svelte';
	import Tabs from '#lib/components/ui/Tabs.svelte';
	import ThemeToggle from '#lib/components/ui/ThemeToggle.svelte';
	import Tooltip from '#lib/components/ui/Tooltip.svelte';
	import type { Tone } from '#lib/components/ui/status.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	const tones: Tone[] = ['neutral', 'accent', 'success', 'warning', 'danger', 'info'];
	const swatches = [
		'bg',
		'surface',
		'surface-sunken',
		'border',
		'border-strong',
		'text',
		'text-muted',
		'text-subtle',
		'accent',
		'accent-soft',
		'success-bg',
		'warning-bg',
		'danger-bg',
		'info-bg'
	];
	const sizes = ['xs', 'sm', 'base', 'md', 'lg', 'xl', '2xl'];
	const spaces = [1, 2, 3, 4, 5, 6, 7, 8];
	const radii = ['sm', 'md', 'lg', 'full'];
	const shadows = ['sm', 'md', 'lg'];

	let text = $state('');
	let area = $state('Some notes\nover two lines');
	let fruit = $state('pear');
	let picked = $state<string[]>(['apple']);
	let on = $state(true);
	let mode = $state('a');
	let tab = $state('overview');
	let confirmOpen = $state(false);
	let dialogOpen = $state(false);
	let loading = $state(false);

	const fruits = [
		{ value: 'apple', label: 'Apple', group: 'Orchard' },
		{ value: 'pear', label: 'Pear', group: 'Orchard', description: 'in season' },
		{ value: 'fig', label: 'Fig', group: 'Tropical', disabled: true, description: 'out of stock' }
	];

	interface Row {
		id: number;
		name: string;
		state: string;
		when: string;
	}
	const rows: Row[] = [
		{ id: 1, name: 'Fix flaky login test', state: 'AwaitingApproval', when: '2026-10-09 14:02' },
		{ id: 2, name: 'Upgrade bits-ui', state: 'Implementing', when: '2026-10-09 13:40' },
		{ id: 3, name: 'Docs typo', state: 'Done', when: '2026-10-08 09:12' },
		{ id: 4, name: 'Broken deploy hook', state: 'Failed', when: '2026-10-07 18:30' }
	];
	const columns: Column<Row>[] = [
		{ id: 'name', header: 'Item', cell: nameCell },
		{ id: 'state', header: 'State', cell: stateCell },
		{ id: 'when', header: 'Updated', cell: whenCell }
	];

	const cond = (type: string, status: string, message = '') => ({
		type,
		status,
		reason: 'X',
		message
	});
	const bootConditions = [
		cond('TenantReady', 'True'),
		cond('NamespaceReady', 'False', 'creating namespace')
	];
	const failedConditions = [
		cond('TenantReady', 'True'),
		cond('NamespaceReady', 'False', 'quota exceeded')
	];
	const at = '2026-10-08T12:00:00.000Z';
	const transitions = [
		{ id: 1, fromState: null, toState: 'Triaging', eventType: 'WorkItemOpened', at },
		{ id: 2, fromState: 'Triaging', toState: 'Planning', eventType: 'JobSucceeded', at },
		{ id: 3, fromState: 'Planning', toState: 'AwaitingApproval', eventType: 'JobSucceeded', at }
	];

	async function fakeConfirm() {
		await new Promise((r) => setTimeout(r, 600));
	}
</script>

{#snippet nameCell(r: Row)}<a href="#row-{r.id}">{r.name}</a>{/snippet}
{#snippet stateCell(r: Row)}<WorkStateChip state={r.state} />{/snippet}
{#snippet whenCell(r: Row)}<time>{r.when}</time>{/snippet}
{#snippet rowActions(r: Row)}
	<Button size="sm" variant="ghost">View</Button>
	<Button size="sm" variant="ghost" aria-label="Cancel {r.name}"><Icon name="x" /></Button>
{/snippet}
{#snippet overview()}<p>Overview panel: the selected tab's content.</p>{/snippet}
{#snippet sandbox()}<p>Sandbox panel.</p>{/snippet}

{#snippet block(title: string, body: import('svelte').Snippet)}
	<section class="ez-stack" style="--ez-gap: var(--ez-space-3)">
		<h3 class="ez-gallery__heading">{title}</h3>
		{@render body()}
	</section>
{/snippet}

{#snippet tokens()}
	<div class="ez-gallery__swatches">
		{#each swatches as s (s)}
			<div class="ez-gallery__swatch" style="background: var(--ez-{s}); color: var(--ez-text)">
				{s}
			</div>
		{/each}
	</div>
	<div class="ez-stack" style="--ez-gap: var(--ez-space-1)">
		{#each sizes as s (s)}
			<div style="font-size: var(--ez-text-{s})">Type scale {s} — quick brown fox</div>
		{/each}
		<div style="font-weight: var(--ez-weight-semibold)">Semibold</div>
		<div style="font-weight: var(--ez-weight-medium)">Medium</div>
		<code>mono: cargo test --workspace</code>
	</div>
	<div class="ez-cluster" style="--ez-gap: var(--ez-space-3)">
		{#each spaces as s (s)}
			<div
				style="width: var(--ez-space-{s}); height: var(--ez-space-{s}); background: var(--ez-accent)"
				title="space-{s}"
			></div>
		{/each}
	</div>
	<div class="ez-cluster" style="--ez-gap: var(--ez-space-3)">
		{#each radii as r (r)}
			<div
				style="width: 3rem; height: 3rem; border: 1px solid var(--ez-border-strong); border-radius: var(--ez-radius-{r}); background: var(--ez-surface)"
				title="radius-{r}"
			></div>
		{/each}
		{#each shadows as s (s)}
			<div
				style="width: 3rem; height: 3rem; border-radius: var(--ez-radius-md); background: var(--ez-surface); box-shadow: var(--ez-shadow-{s})"
				title="shadow-{s}"
			></div>
		{/each}
	</div>
{/snippet}

{#snippet buttons()}
	<div class="ez-cluster">
		<Button variant="primary">Primary</Button>
		<Button variant="secondary">Secondary</Button>
		<Button variant="ghost">Ghost</Button>
		<Button variant="danger">Danger</Button>
		<Button variant="primary" disabled>Disabled</Button>
	</div>
	<div class="ez-cluster">
		<Button variant="primary" size="sm">Small</Button>
		<Button size="sm">Small</Button>
		<Button icon aria-label="Add"><Icon name="plus" /></Button>
		<Button icon variant="ghost" aria-label="Close"><Icon name="x" /></Button>
		<Button
			variant="primary"
			{loading}
			onclick={() => {
				loading = true;
				setTimeout(() => (loading = false), 1500);
			}}>Click to load</Button
		>
		<Button variant="secondary" loading>Saving</Button>
		<LinkButton href="#link" variant="secondary">Link button</LinkButton>
		<a href="#plain">Plain link</a>
	</div>
{/snippet}

{#snippet badges()}
	<div class="ez-cluster">
		{#each tones as t (t)}<Badge tone={t}>{t}</Badge>{/each}
		<Badge tone="warning" hollow>waiting</Badge>
		<Badge plain>plain</Badge>
	</div>
	<div class="ez-cluster">
		<StatusBadge state="pending" /><StatusBadge state="in-progress" /><StatusBadge state="done" />
		<StatusBadge state="failed" />
	</div>
	<div class="ez-cluster">
		{#each ['Triaging', 'AwaitingApproval', 'Implementing', 'Done', 'Failed', 'Cancelled'] as s (s)}
			<WorkStateChip state={s} />
		{/each}
	</div>
{/snippet}

{#snippet forms()}
	<TextField label="Text field" placeholder="Type here" bind:value={text} help="Helper text." />
	<TextField
		label="With an error"
		value="Bad Name"
		issues={[{ message: 'Use lowercase letters.' }]}
	/>
	<TextField label="Disabled" value="read only" disabled />
	<TextArea label="Text area" bind:value={area} />
	<Select label="Select" options={fruits} bind:value={fruit} />
	<Combobox
		label="Combobox (multi)"
		options={fruits}
		bind:value={picked}
		help="Search to filter."
	/>
	<FieldSet legend="Choices">
		<Checkbox label="Checked" checked />
		<Checkbox label="Unchecked" help="With help text." />
		<Checkbox label="Invalid" issues={[{ message: 'Required.' }]} />
		<Checkbox label="Disabled" disabled />
		<Switch label="Switch" bind:checked={on} />
		<Switch label="Disabled switch" disabled />
	</FieldSet>
	<RadioGroup
		label="Radio group"
		bind:value={mode}
		options={[
			{ value: 'a', label: 'Option A', description: 'With a description.' },
			{ value: 'b', label: 'Option B' }
		]}
	/>
{/snippet}

{#snippet notices()}
	<Notice tone="info" title="Install the GitHub App"
		>Info with a title and actions.
		{#snippet actions()}<Button variant="primary" size="sm">Install</Button>{/snippet}
	</Notice>
	<Notice tone="success">Saved.</Notice>
	<Notice tone="warning" title="Quota almost reached">Warning text.</Notice>
	<Notice tone="error">Something went wrong.</Notice>
{/snippet}

{#snippet toastDemo()}
	<div class="ez-cluster">
		<Button onclick={() => toasts.push('Saved', { tone: 'success' })}>Success toast</Button>
		<Button onclick={() => toasts.push('Heads up', { tone: 'info', description: 'Details here.' })}
			>Info toast</Button
		>
		<Button onclick={() => toasts.push('Quota low', { tone: 'warning' })}>Warning toast</Button>
		<Button variant="danger" onclick={() => toasts.push('Could not save', { tone: 'error' })}
			>Error toast</Button
		>
	</div>
	<div class="ez-toast ez-toast--success" style="width: 100%">
		<span class="ez-toast__icon"><Icon name="check" /></span>
		<div>
			<div class="ez-toast__title">Saved</div>
			<div class="ez-toast__description">Static preview</div>
		</div>
		<span></span>
	</div>
	<div class="ez-toast ez-toast--error" style="width: 100%">
		<span class="ez-toast__icon"><Icon name="alert-circle" /></span>
		<div><div class="ez-toast__title">Could not save</div></div>
		<span></span>
	</div>
{/snippet}

{#snippet overlays()}
	<div class="ez-cluster">
		<Button onclick={() => (dialogOpen = true)}>Open dialog</Button>
		<Button variant="danger" onclick={() => (confirmOpen = true)}>Open confirm dialog</Button>
		<DropdownMenu
			label="Menu"
			entries={[
				{ type: 'label', text: 'Actions' },
				{ type: 'item', label: 'Rename', onselect: () => toasts.push('Rename') },
				{ type: 'item', label: 'Open docs', href: '#docs', icon: 'external' },
				{ type: 'separator' },
				{ type: 'item', label: 'Delete', icon: 'trash', tone: 'danger' }
			]}
		>
			{#snippet trigger()}Menu <Icon name="chevron-down" />{/snippet}
		</DropdownMenu>
		<Tooltip text="Copy the identifier">
			{#snippet trigger(props)}
				<Button {...props} icon aria-label="Copy"><Icon name="plus" /></Button>
			{/snippet}
		</Tooltip>
	</div>
	<Dialog bind:open={dialogOpen} title="Dialog" description="Focus is trapped; Escape closes.">
		<p>Body content.</p>
		{#snippet footer()}<Button variant="primary" onclick={() => (dialogOpen = false)}>Done</Button
			>{/snippet}
	</Dialog>
	<ConfirmDialog
		bind:open={confirmOpen}
		title="Delete project?"
		description="This cannot be undone."
		confirmLabel="Delete"
		onconfirm={fakeConfirm}
	/>
	<div
		class="ez-dialog__content ez-dialog__content--static"
		role="group"
		aria-label="Dialog preview"
	>
		<div>
			<div class="ez-dialog__title">Delete project?</div>
			<p class="ez-dialog__description">Static preview of the dialog surface.</p>
		</div>
		<div class="ez-dialog__footer">
			<Button>Cancel</Button><Button variant="danger">Delete</Button>
		</div>
	</div>
	<div class="ez-menu" role="group" aria-label="Menu preview" style="width: 14rem">
		<div class="ez-menu__header">
			<strong>Curtis Ruck</strong><span class="ez-muted">@ruckc</span>
		</div>
		<div class="ez-menu__label">Theme</div>
		<div class="ez-menu__item ez-menu__item--checked" data-highlighted>
			<Icon name="monitor" />System<span class="ez-menu__check"><Icon name="check" /></span>
		</div>
		<div class="ez-menu__item"><Icon name="sun" />Light</div>
		<div class="ez-menu__separator"></div>
		<div class="ez-menu__item ez-menu__item--danger"><Icon name="logout" />Sign out</div>
	</div>
	<div><span class="ez-tooltip" role="presentation">Tooltip text</span></div>
{/snippet}

{#snippet tabsDemo()}
	<Tabs
		label="Example tabs"
		bind:value={tab}
		tabs={[
			{ value: 'overview', label: 'Overview' },
			{ value: 'sandbox', label: 'Sandbox' },
			{ value: 'off', label: 'Disabled', disabled: true }
		]}
	>
		{#snippet panel(value)}
			{#if value === 'overview'}{@render overview()}{:else}{@render sandbox()}{/if}
		{/snippet}
	</Tabs>
{/snippet}

{#snippet tables()}
	<DataTable label="Work items" items={rows} {columns} rowKey={(r) => r.id} {rowActions} />
	<DataTable
		label="Empty"
		items={[]}
		{columns}
		rowKey={(r: Row) => r.id}
		emptyTitle="No work items yet"
		emptyDescription="Work appears here when an issue reaches a project."
	>
		{#snippet emptyActions()}<Button variant="primary" size="sm">Create project</Button>{/snippet}
	</DataTable>
	<DataTable label="Loading" items={[]} {columns} rowKey={(r: Row) => r.id} loading />
{/snippet}

{#snippet steps()}
	<ConditionChecklist conditions={bootConditions} />
	<ConditionChecklist conditions={failedConditions} />
	<WorkTimeline state="AwaitingApproval" {transitions} />
	<StepList
		label="All states"
		items={[
			{ id: 1, status: 'done', label: 'Done step', stateText: 'Done' },
			{ id: 2, status: 'active', label: 'Active step', stateText: 'In progress' },
			{ id: 3, status: 'pending', label: 'Pending step', stateText: 'Pending' },
			{
				id: 4,
				status: 'failed',
				label: 'Failed step',
				stateText: 'Failed',
				message: 'connection refused'
			}
		]}
	/>
{/snippet}

{#snippet layoutPieces()}
	<PageHeader title="Page header" description="A description line under the title.">
		{#snippet meta()}<WorkStateChip state="Implementing" /><code>acme/web#12</code>{/snippet}
		{#snippet actions()}<Button variant="primary">Primary action</Button><Button>Secondary</Button
			>{/snippet}
	</PageHeader>
	<Section title="Section" description="A titled block in a card.">
		{#snippet actions()}<Button size="sm">Action</Button>{/snippet}
		<KeyValueList
			items={[
				{ term: 'Namespace', value: 'proj-alpha' },
				{ term: 'Owner', value: 'ruckc' },
				{ term: 'Status', children: kvStatus }
			]}
		/>
	</Section>
	<Card sunken>A sunken card.</Card>
	<EmptyState title="Nothing here" description="An empty state explains why and what to do next." />
	<Skeleton label="Loading list" />
	<div class="ez-cluster">
		<Avatar login="ruckc" url={null} />
		<ThemeToggle />
	</div>
{/snippet}
{#snippet kvStatus()}<StatusBadge state="done" />{/snippet}

{@render block('Tokens', tokens)}
{@render block('Buttons and links', buttons)}
{@render block('Badges and status chips', badges)}
{@render block('Form controls', forms)}
{@render block('Notices', notices)}
{@render block('Toasts', toastDemo)}
{@render block('Dialog, menu and tooltip', overlays)}
{@render block('Tabs', tabsDemo)}
{@render block('Tables and lists', tables)}
{@render block('Step checklists', steps)}
{@render block('Page, section and card', layoutPieces)}
