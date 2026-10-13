/** The ACP session id per project, kept in `sessionStorage` so a reload can resume. Not a secret. */
const key = (project: string) => `enzarb.chat.${project}`;

/** The ids the sandbox issues: short, printable, no whitespace, slashes or control characters. */
const VALID_ID = /^[A-Za-z0-9._:-]{1,128}$/;

export const isValidSessionId = (id: unknown): id is string =>
	typeof id === 'string' && VALID_ID.test(id);

function store(): Storage | null {
	try {
		return typeof sessionStorage === 'undefined' ? null : sessionStorage;
	} catch {
		return null;
	}
}

export function load(project: string): string | undefined {
	try {
		const s = store();
		const v = s?.getItem(key(project));
		if (v == null) return undefined;
		if (isValidSessionId(v)) return v;
		// Tampered or corrupt: never offer it as `open.resume`, and drop it.
		s?.removeItem(key(project));
		return undefined;
	} catch {
		return undefined;
	}
}

export function save(project: string, id: string): void {
	if (!isValidSessionId(id)) return;
	try {
		store()?.setItem(key(project), id);
	} catch {
		// Storage disabled or full: the chat just won't resume.
	}
}

export function clear(project: string): void {
	try {
		store()?.removeItem(key(project));
	} catch {
		// Nothing to clear.
	}
}
