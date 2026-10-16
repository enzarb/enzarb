/// <reference no-default-lib="true"/>
/// <reference lib="esnext" />
/// <reference lib="webworker" />

import { assets, immutable } from '$app/manifest';
import { self as sw } from '$app/service-worker';
import { handlePush, OFFLINE_PATH, openOrFocus, parsePush, planFetch } from '#lib/sw-handlers.ts';

// Static shell only: hashed build assets, files from `static/` and the prerendered /offline page.
// Navigations always go to the network first and are never stored, so no authenticated HTML,
// remote function response or `/live/` frame is ever cached.
// Manifest paths are relative to the base (no leading slash); requests are matched by pathname.
const abs = (path: string) => (path.startsWith('/') ? path : `/${path}`);
const files = [
	...[...immutable, ...assets].map((f) => abs(f.path)).filter((p) => p !== '/service-worker.js'),
	OFFLINE_PATH
];
const precached = new Set<string>(files);

// Build output file names are content-hashed, so their list identifies the build. (The kit
// version name is not available inside the worker.)
const CACHE = `enzarb-shell-${hash(files.join('\n'))}`;

function hash(s: string): string {
	let h = 5381;
	for (let i = 0; i < s.length; i++) h = ((h * 33) ^ s.charCodeAt(i)) >>> 0;
	return h.toString(36);
}

// No skipWaiting(): a new worker activates once the old one has no clients left, so it never
// swaps assets under a page that the version check (versions.ts / kit `version.pollInterval`)
// is about to reload anyway.
sw.addEventListener('install', (event) => {
	event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(files)));
});

sw.addEventListener('activate', (event) => {
	event.waitUntil(
		caches
			.keys()
			.then((keys) =>
				Promise.all(
					keys
						.filter((k) => k.startsWith('enzarb-shell-') && k !== CACHE)
						.map((k) => caches.delete(k))
				)
			)
	);
});

sw.addEventListener('fetch', (event) => {
	const plan = planFetch(event.request, sw.location.origin, precached);
	if (plan === 'bypass') return;
	if (plan === 'asset') {
		event.respondWith(
			caches
				.open(CACHE)
				.then(async (cache) => (await cache.match(event.request)) ?? fetch(event.request))
		);
		return;
	}
	event.respondWith(
		fetch(event.request).catch(async () => {
			const cache = await caches.open(CACHE);
			return (await cache.match(OFFLINE_PATH)) ?? Response.error();
		})
	);
});

sw.addEventListener('push', (event) => {
	event.waitUntil(handlePush(sw.registration, parsePush(event.data ? event.data.text() : null)));
});

sw.addEventListener('notificationclick', (event) => {
	event.notification.close();
	event.waitUntil(openOrFocus(sw.clients, event.notification.data, sw.location.origin));
});
