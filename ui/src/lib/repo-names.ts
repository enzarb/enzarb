/** GitHub repository rules shared by the form (early feedback) and the server (the real check). */

const NAME_RE = /^[A-Za-z0-9._-]{1,100}$/;
export const MAX_DESCRIPTION_LENGTH = 350;

/** Why `name` cannot be a new repository name, or null when it can. */
export function validateRepoName(name: string): string | null {
	if (!name) return 'Enter a repository name';
	if (name === '.' || name === '..') return 'A repository cannot be named "." or ".."';
	if (name.length > 100) return 'At most 100 characters';
	if (!NAME_RE.test(name)) return 'Use only letters, digits, ".", "-" and "_" (no "/" or spaces)';
	return null;
}

/** Why `description` is not acceptable, or null (empty is fine). */
export function validateDescription(description: string): string | null {
	if (description.length > MAX_DESCRIPTION_LENGTH)
		return `At most ${MAX_DESCRIPTION_LENGTH} characters`;
	// eslint-disable-next-line no-control-regex
	if (/[\u0000-\u001f\u007f]/.test(description)) return 'Control characters are not allowed';
	return null;
}

/** GitHub logins are case-insensitive. */
export function sameOwner(a: string, b: string): boolean {
	return a.toLowerCase() === b.toLowerCase();
}

/** Result codes of the create-repository callback; the only thing passed back in the URL. */
export const REPO_RESULTS = [
	'created',
	'created-not-installed',
	'identity-mismatch',
	'owner-not-allowed',
	'forbidden',
	'name-taken',
	'invalid',
	'timeout',
	'failed'
] as const;
export type RepoResult = (typeof REPO_RESULTS)[number];

export function parseRepoResult(value: string | null | undefined): RepoResult | null {
	return (REPO_RESULTS as readonly string[]).includes(value ?? '') ? (value as RepoResult) : null;
}
