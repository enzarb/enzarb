// Browser notifications for agent events (opt-in). The preference lives in
// localStorage; a notification only fires when the user has granted permission
// and the tab isn't focused — if they're already looking at the timeline
// there's nothing to announce.
const PREF_KEY = 'enzarb:agent-notifications';

export function notificationsSupported(): boolean {
	return typeof window !== 'undefined' && 'Notification' in window;
}

export function notificationsEnabled(): boolean {
	if (!notificationsSupported()) return false;
	return localStorage.getItem(PREF_KEY) === 'on' && Notification.permission === 'granted';
}

// Returns whether notifications ended up enabled (permission may be denied).
export async function setNotificationsEnabled(on: boolean): Promise<boolean> {
	if (!notificationsSupported()) return false;
	if (!on) {
		localStorage.setItem(PREF_KEY, 'off');
		return false;
	}
	const perm =
		Notification.permission === 'granted' ? 'granted' : await Notification.requestPermission();
	localStorage.setItem(PREF_KEY, perm === 'granted' ? 'on' : 'off');
	return perm === 'granted';
}

export function notify(title: string, body: string, tag?: string): void {
	if (!notificationsEnabled()) return;
	if (!document.hidden && document.hasFocus()) return;
	try {
		const n = new Notification(title, {
			body: body.length > 180 ? body.slice(0, 177) + '…' : body,
			tag
		});
		n.onclick = () => {
			window.focus();
			n.close();
		};
	} catch {
		// Notification construction can throw (e.g. on some mobile browsers).
	}
}

// Heuristic for "the agent asked the user something": there is no explicit
// question event in the ACP stream, so inspect the tail of the assistant's
// final message for question-like text.
export function looksLikeQuestion(text: string): boolean {
	const lines = text
		.trim()
		.split('\n')
		.map((l) => l.trim())
		.filter((l) => l && !l.startsWith('```'));
	if (lines.slice(-3).some((l) => l.endsWith('?'))) return true;
	const tail = lines.slice(-2).join(' ');
	return /\b(should i|do you want|would you like|let me know|which (one|option|approach)|please confirm|your (call|preference)|wdyt)\b/i.test(
		tail
	);
}
