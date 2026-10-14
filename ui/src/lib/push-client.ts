import { urlBase64ToUint8Array } from '#lib/push.ts';

export type PushSupport =
	/** Everything needed is there. */
	| 'ok'
	/** iOS/iPadOS only delivers Web Push to an app added to the home screen (16.4+). */
	| 'needs-install'
	| 'unsupported'
	| 'denied';

export interface PushEnv {
	hasServiceWorker: boolean;
	hasPushManager: boolean;
	hasNotification: boolean;
	permission: NotificationPermission | 'unavailable';
	isApple: boolean;
	standalone: boolean;
}

export function detectSupport(env: PushEnv): PushSupport {
	if (!env.hasServiceWorker)
		return env.isApple && !env.standalone ? 'needs-install' : 'unsupported';
	if (!env.hasPushManager || !env.hasNotification) {
		return env.isApple && !env.standalone ? 'needs-install' : 'unsupported';
	}
	if (env.permission === 'denied') return 'denied';
	return 'ok';
}

/** Reads the capabilities of the running browser. */
export function browserEnv(): PushEnv {
	const ua = navigator.userAgent;
	// iPadOS reports a desktop Mac user agent; touch points tell them apart.
	const isApple =
		/iPhone|iPad|iPod/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1);
	return {
		hasServiceWorker: 'serviceWorker' in navigator,
		hasPushManager: 'PushManager' in window,
		hasNotification: 'Notification' in window,
		permission: 'Notification' in window ? Notification.permission : 'unavailable',
		isApple,
		standalone:
			window.matchMedia('(display-mode: standalone)').matches ||
			(navigator as Navigator & { standalone?: boolean }).standalone === true
	};
}

export interface DeviceSubscription {
	endpoint: string;
	keys: { p256dh: string; auth: string };
}

/** This browser's current push subscription, if it has one. */
export async function currentSubscription(): Promise<PushSubscription | null> {
	const reg = await navigator.serviceWorker.ready;
	return reg.pushManager.getSubscription();
}

/** Asks for permission and subscribes this browser with the server's public key. */
export async function subscribeThisDevice(publicKey: string): Promise<DeviceSubscription> {
	const permission = await Notification.requestPermission();
	if (permission !== 'granted') throw new Error('Notification permission was not granted');
	const reg = await navigator.serviceWorker.ready;
	const sub =
		(await reg.pushManager.getSubscription()) ??
		(await reg.pushManager.subscribe({
			userVisibleOnly: true,
			applicationServerKey: urlBase64ToUint8Array(publicKey)
		}));
	const json = sub.toJSON();
	if (!json.endpoint || !json.keys?.p256dh || !json.keys.auth) {
		throw new Error('The browser returned an incomplete subscription');
	}
	return { endpoint: json.endpoint, keys: { p256dh: json.keys.p256dh, auth: json.keys.auth } };
}

export async function unsubscribeThisDevice(): Promise<void> {
	await (await currentSubscription())?.unsubscribe();
}
