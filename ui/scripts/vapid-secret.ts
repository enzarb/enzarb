// Creates the Web Push (VAPID) keypair Secret once, if it does not exist yet. Run by the chart's
// `enzarb-vapid-key` hook Job (pre-install / pre-upgrade) with `node scripts/vapid-secret.ts`.
// An existing Secret is never touched, so the keys stay stable across upgrades (subscriptions are
// bound to the public key).
//
// Env: ENZARB_VAPID_SECRET (Secret name), ENZARB_VAPID_NAMESPACE (default: the in-cluster namespace).
import { ApiException, CoreV1Api, KubeConfig } from '@kubernetes/client-node';
import webpush from 'web-push';

const name = process.env.ENZARB_VAPID_SECRET;
if (!name) throw new Error('ENZARB_VAPID_SECRET is required');

const kc = new KubeConfig();
kc.loadFromDefault();
const namespace =
	process.env.ENZARB_VAPID_NAMESPACE ||
	kc.getContextObject(kc.getCurrentContext())?.namespace ||
	'default';
const api = kc.makeApiClient(CoreV1Api);

try {
	await api.readNamespacedSecret({ name, namespace });
	console.log(`vapid: secret ${namespace}/${name} exists, keeping it`);
} catch (e) {
	if (!(e instanceof ApiException) || e.code !== 404) throw e;
	const keys = webpush.generateVAPIDKeys();
	try {
		await api.createNamespacedSecret({
			namespace,
			body: {
				metadata: {
					name,
					annotations: { 'helm.sh/resource-policy': 'keep' },
					labels: { 'app.kubernetes.io/managed-by': 'enzarb-vapid-job' }
				},
				type: 'Opaque',
				stringData: { 'public-key': keys.publicKey, 'private-key': keys.privateKey }
			}
		});
		console.log(`vapid: created secret ${namespace}/${name}`);
	} catch (err) {
		// Another run created it first: fine.
		if (!(err instanceof ApiException) || err.code !== 409) throw err;
		console.log(`vapid: secret ${namespace}/${name} appeared meanwhile, keeping it`);
	}
}
