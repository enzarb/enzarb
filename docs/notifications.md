# Notifications and the installable app

enzarb is an installable web app (PWA) and can register your devices for push notifications. The infrastructure (subscriptions, VAPID keypair, test notification) is in place for generic use; no enzarb event is turned into a notification at the moment ([ADR-0027](adr/0027-workflow-clean-slate.md)). Interfaces and storage: [contracts.md](contracts.md) section 14.

## Turn notifications on

1. Open **Settings**, section **Notifications**, on the device you want to notify.
2. Choose **Turn on for this device** and allow notifications when the browser asks.
3. **Send a test** to check that it arrives. **Turn off for this device** removes the device. Other devices of yours are listed and can be removed there too.

Each device is set up separately. A notification never contains code, logs or tokens.

## Install the app

Chrome, Edge and Android browsers offer **Install the app** in Settings once the browser allows it; dismissing it hides it for good on that browser. You can also use the browser's own menu (Install app / Add to Home screen).

## iPhone and iPad

Safari delivers push notifications only to a web app added to the home screen, on **iOS/iPadOS 16.4 or later**. Tap Share, **Add to Home Screen**, open enzarb from the new icon, then turn notifications on in Settings from there. In a normal Safari tab the Settings section explains this instead of offering the switch.

## Troubleshooting

- "Notifications are blocked": allow them for this site in the browser's site settings, then reload.
- "Not configured on this server": the operator has not provisioned the VAPID keypair (see `helm/README.md`, "Web Push keypair").
- Nothing arrives on a device that was offline for a long time or whose browser data was cleared: turn notifications off and on again for that device. Devices the push service reports as gone are removed automatically.
