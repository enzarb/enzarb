import webpush from 'web-push';
import type { PushPayload } from '#lib/push.ts';
import { getConfig, type VapidConfig } from './config.ts';
import { getSql } from './db.ts';
import { pushStore, type PushStore, type PushTarget } from './push-store.ts';

export type SendResult = 'ok' | 'gone' | 'error';
export type Send = (target: PushTarget, payload: PushPayload) => Promise<SendResult>;

/** Seconds a push service may hold a message for an offline device. */
const TTL_SECS = 3600;

/**
 * Sends one encrypted message through the device's push service. The VAPID private key stays in
 * this module (server only). 404 and 410 mean the subscription is gone for good.
 */
export function createSend(vapid: VapidConfig): Send {
	return async (target, payload) => {
		try {
			await webpush.sendNotification(
				{ endpoint: target.endpoint, keys: { p256dh: target.p256dh, auth: target.auth } },
				JSON.stringify(payload),
				{
					TTL: TTL_SECS,
					vapidDetails: {
						subject: vapid.subject,
						publicKey: vapid.publicKey,
						privateKey: vapid.privateKey
					}
				}
			);
			return 'ok';
		} catch (e) {
			const status = (e as { statusCode?: unknown }).statusCode;
			if (status === 404 || status === 410) return 'gone';
			// Never log the endpoint: it is a capability URL.
			console.warn(`push: send failed (${typeof status === 'number' ? status : 'network'})`);
			return 'error';
		}
	};
}

/**
 * Sends `payload` to every target; removes subscriptions the push service reports as gone and
 * records the last use of the ones that worked. Returns how many devices accepted it.
 */
export async function deliver(
	store: Pick<PushStore, 'remove' | 'touch'>,
	send: Send,
	owner: string,
	targets: PushTarget[],
	payload: PushPayload
): Promise<number> {
	let ok = 0;
	await Promise.all(
		targets.map(async (t) => {
			const result = await send(t, payload);
			if (result === 'gone') await store.remove(owner, t.endpoint);
			else if (result === 'ok') {
				ok++;
				await store.touch(owner, t.endpoint);
			}
		})
	);
	return ok;
}

/** The public key for the browser, or null when push is not configured. */
export const vapidPublicKey = (): string | null => getConfig().vapid?.publicKey ?? null;

let sender: Send | undefined;

/** Lazily created sender, or null when no VAPID keys are configured. */
export function getSend(): Send | null {
	const vapid = getConfig().vapid;
	if (!vapid) return null;
	return (sender ??= createSend(vapid));
}

export const getPushStore = () => pushStore(getSql());
