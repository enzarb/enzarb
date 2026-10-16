import * as v from 'valibot';

/**
 * Device subscription categories. Since ADR-0022 there is one: the engine's `needs_you`. Older
 * devices may still hold the former per-state names; the notifier sends to every device.
 */
export const NEEDS_YOU_CATEGORIES = ['needs_you'] as const;
export type NeedsYouCategory = (typeof NEEDS_YOU_CATEGORIES)[number];
const LEGACY_CATEGORIES = [
	'approval',
	'merge',
	'review',
	'decision',
	'blocked',
	'failed',
	'question'
] as const;

/** What the push service carries to the device (and the service worker shows): nothing else. */
export interface PushPayload {
	title: string;
	body: string;
	/** In-app path, always relative (`/task/12`). */
	url: string;
	/** Same tag replaces the earlier notification on the device. */
	tag: string;
}

export const CATEGORY_LABELS: Record<NeedsYouCategory, string> = {
	needs_you: 'A task is waiting on you'
};

export const BODY_MAX = 120;

const truncate = (s: string, max: number) => {
	const t = s.replace(/\s+/g, ' ').trim();
	return t.length <= max ? t : `${t.slice(0, max - 1).trimEnd()}…`;
};

/** Data-only push: close the notifications with this tag on the device, show nothing. */
export interface ClearPayload {
	type: 'clear';
	tag: string;
}

export type PushMessage = PushPayload | ClearPayload;

/** One tag per task, so a newer notification replaces the older one and a clear can find it. */
export const itemTag = (id: number): string => `item-${id}`;

/** The "Waiting on you" page with the card highlighted. */
export function highlightUrl(item: { id: number }): string {
	return `/tasks?${new URLSearchParams({ highlight: String(item.id) })}`;
}

/**
 * The notification for one task. Built only from the id, the display title and the projection's
 * reason (a short label the workflow declares): never task state, tokens, code or logs.
 */
export function pushPayload(item: {
	id: number;
	title: string;
	reason?: string | null;
}): PushPayload {
	return {
		title: truncate(item.reason?.trim() || 'A task is waiting on you', BODY_MAX),
		body: truncate(item.title, BODY_MAX),
		url: highlightUrl(item),
		tag: itemTag(item.id)
	};
}

export const clearPayload = (id: number): ClearPayload => ({ type: 'clear', tag: itemTag(id) });

/** One notification standing in for a burst; replaces itself through its tag. */
export function summaryPayload(count: number): PushPayload {
	return {
		title: `${count} tasks need you`,
		body: 'Open the task list to see what is waiting.',
		url: '/tasks',
		tag: 'work-summary'
	};
}

export function testPayload(): PushPayload {
	return {
		title: 'Notifications are on',
		body: 'This is a test notification from enzarb.',
		url: '/settings',
		tag: 'test'
	};
}

export const CategoriesSchema = v.pipe(
	v.array(v.picklist([...NEEDS_YOU_CATEGORIES, ...LEGACY_CATEGORIES])),
	v.maxLength(NEEDS_YOU_CATEGORIES.length + LEGACY_CATEGORIES.length)
);

export const SubscriptionSchema = v.object({
	endpoint: v.pipe(v.string(), v.url(), v.maxLength(2048), v.startsWith('https://')),
	keys: v.object({
		p256dh: v.pipe(v.string(), v.minLength(1), v.maxLength(256)),
		auth: v.pipe(v.string(), v.minLength(1), v.maxLength(256))
	}),
	categories: CategoriesSchema
});

export const EndpointSchema = v.pipe(v.string(), v.maxLength(2048));

export const UpdateCategoriesSchema = v.object({
	endpoint: EndpointSchema,
	categories: CategoriesSchema
});

/** Applications server key (base64url) -> bytes for `PushManager.subscribe`. */
export function urlBase64ToUint8Array(b64: string): Uint8Array<ArrayBuffer> {
	const padded = (b64 + '='.repeat((4 - (b64.length % 4)) % 4))
		.replace(/-/g, '+')
		.replace(/_/g, '/');
	const raw = atob(padded);
	const out = new Uint8Array(new ArrayBuffer(raw.length));
	for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
	return out;
}
