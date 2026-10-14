import { error } from '@sveltejs/kit';
import { command, getRequestEvent, query } from '$app/server';
import {
	EndpointSchema,
	SubscriptionSchema,
	UpdateCategoriesSchema,
	testPayload
} from '#lib/push.ts';
import { deliver, getPushStore, getSend, vapidPublicKey } from '#lib/server/push.ts';

/** Minimum time between test notifications per user (per pod). */
const TEST_COOLDOWN_MS = 10_000;
const lastTest = new Map<string, number>();

function login(): string {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	return user.login;
}

/**
 * Whether push is configured, the public VAPID key (never the private one) and the signed-in
 * user's own devices. No keys are returned for the devices.
 */
export const getPushState = query(async () => {
	const owner = login();
	return {
		publicKey: vapidPublicKey(),
		devices: await getPushStore().list(owner)
	};
});

/** Registers this device for the signed-in user. The owner is always the session login. */
export const subscribe = command(SubscriptionSchema, async (s) => {
	const owner = login();
	if (!vapidPublicKey()) error(503, 'Push notifications are not configured');
	const userAgent = getRequestEvent().request.headers.get('user-agent')?.slice(0, 300) ?? null;
	const ok = await getPushStore().upsert(owner, {
		endpoint: s.endpoint,
		p256dh: s.keys.p256dh,
		auth: s.keys.auth,
		categories: s.categories,
		userAgent
	});
	// The endpoint belongs to another user: do not reveal that, and change nothing.
	if (!ok) error(409, 'This device could not be registered');
});

export const updateCategories = command(
	UpdateCategoriesSchema,
	async ({ endpoint, categories }) => {
		const owner = login();
		if (!(await getPushStore().setCategories(owner, endpoint, categories)))
			error(404, 'Device not found');
	}
);

export const unsubscribe = command(EndpointSchema, async (endpoint) => {
	await getPushStore().remove(login(), endpoint);
});

/** Sends a test notification to one of the user's own devices. */
export const sendTest = command(EndpointSchema, async (endpoint) => {
	const owner = login();
	const send = getSend();
	if (!send) error(503, 'Push notifications are not configured');
	const now = Date.now();
	if (now - (lastTest.get(owner) ?? 0) < TEST_COOLDOWN_MS) error(429, 'Wait a moment');
	lastTest.set(owner, now);
	const store = getPushStore();
	const target = (await store.targets(owner, null)).find((t) => t.endpoint === endpoint);
	if (!target) error(404, 'Device not found');
	const sent = await deliver(store, send, owner, [target], testPayload());
	if (sent === 0) error(502, 'The push service did not accept the notification');
});
