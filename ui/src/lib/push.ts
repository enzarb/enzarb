import * as v from 'valibot';
import { NEEDS_YOU_CATEGORIES, type NeedsYouCategory } from '#lib/work-items.ts';

/** What the push service carries to the device (and the service worker shows): nothing else. */
export interface PushPayload {
	title: string;
	body: string;
	/** In-app path, always relative (`/work/12`). */
	url: string;
	/** Same tag replaces the earlier notification on the device. */
	tag: string;
}

export const CATEGORY_LABELS: Record<NeedsYouCategory, string> = {
	approval: 'Plan awaiting approval',
	merge: 'Ready to merge',
	review: 'Review needed',
	decision: 'Decision needed',
	blocked: 'Change request blocked',
	failed: 'Failed or stale'
};

/** Fixed title per category, so no item text ends up in the title. */
const TITLES: Record<NeedsYouCategory, string> = {
	approval: 'Plan awaiting your approval',
	merge: 'Change request ready to merge',
	review: 'Change request awaiting your review',
	decision: 'Decision needed from you',
	blocked: 'Change request blocked',
	failed: 'Work item failed'
};

export const BODY_MAX = 120;

const truncate = (s: string, max: number) => {
	const t = s.replace(/\s+/g, ' ').trim();
	return t.length <= max ? t : `${t.slice(0, max - 1).trimEnd()}…`;
};

/**
 * The notification for one work item. Built only from the item id, its display title and the
 * category: never `state_data`, the failure reason, tokens, code or logs.
 */
export function pushPayload(
	item: { id: number; title: string },
	category: NeedsYouCategory
): PushPayload {
	return {
		title: TITLES[category],
		body: truncate(item.title, BODY_MAX),
		url: `/work/${item.id}`,
		tag: `work-${item.id}`
	};
}

/** One notification standing in for a burst; replaces itself through its tag. */
export function summaryPayload(count: number): PushPayload {
	return {
		title: `${count} items need you`,
		body: 'Open the work queue to see what is waiting.',
		url: '/work',
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
	v.array(v.picklist(NEEDS_YOU_CATEGORIES)),
	v.maxLength(NEEDS_YOU_CATEGORIES.length)
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
