# Notifications and the installable app

enzarb is an installable web app (PWA) and can send push notifications to your devices when something needs you, so you do not have to watch the page. Interfaces and storage: [contracts.md](contracts.md) section 14.

## Turn notifications on

1. Open **Settings**, section **Notifications**, on the device you want to notify.
2. Choose **Turn on for this device** and allow notifications when the browser asks.
3. Tick the events you want: plan awaiting approval, ready to merge, change request blocked, failed or stale (this includes failed runs). All are on by default.
4. **Send a test** to check that it arrives. **Turn off for this device** removes the device. Other devices of yours are listed and can be removed there too.

Each device is set up separately. You are notified only for Projects you own, once per entry into a state that needs you: further events on an item that stays in that state (a failed task records a `Failed` → `Failed` step on every event) do not notify again, and neither does a move to another state of the same kind (`Failed` → `Stale`). Leaving and coming back later notifies again. A burst (more than three in a minute) is collapsed into one "N items need you" notification. A notification contains the kind of event, the item title and a link. It never contains code, logs or tokens. There is no send delay.

**Clicking a notification** opens (or focuses) the Tasks board, filtered to the item's Project, scrolls to the card, focuses it and outlines it with a short pulse (no motion with reduced motion turned on), in whatever column it is in now. If the item moved on since the notification, the card says so in one line, for example "Was waiting for your review — now: Resolving conflicts". If the item is not on the board, a banner links to its task page.

**Notifications clear themselves.** Every notification for a task uses the tag `item-<id>`, so a newer one replaces the older. When an item you were notified about no longer needs you (or now needs you for something else), enzarb sends your devices a silent "clear" message once, and the notification disappears from the tray. A state that lasts seconds (a change request that briefly waits for review and then resolves conflicts) can still notify, but the notification is withdrawn as soon as the item moves on.

Caveat: Chrome expects every push message to show a notification. A clear is only sent to devices that were shown that tag, so normally it closes one; but if you already dismissed it, Chrome may show its own generic "This site has been updated in the background" notice. Clears that reach a device while it is offline are delivered later or dropped after an hour.

## Install the app

Chrome, Edge and Android browsers offer **Install the app** in Settings once the browser allows it; dismissing it hides it for good on that browser. You can also use the browser's own menu (Install app / Add to Home screen).

## iPhone and iPad

Safari delivers push notifications only to a web app added to the home screen, on **iOS/iPadOS 16.4 or later**. Tap Share, **Add to Home Screen**, open enzarb from the new icon, then turn notifications on in Settings from there. In a normal Safari tab the Settings section explains this instead of offering the switch.

## Troubleshooting

- "Notifications are blocked": allow them for this site in the browser's site settings, then reload.
- "Not configured on this server": the operator has not provisioned the VAPID keypair (see `helm/README.md`, "Web Push keypair").
- Nothing arrives on a device that was offline for a long time or whose browser data was cleared: turn notifications off and on again for that device. Devices the push service reports as gone are removed automatically.
