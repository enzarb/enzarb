// Test-only. SvelteKit does not export its remote-form field naming or FormData conversion,
// so tests reach into the package. Everything that does so lives here, pinned to the version
// the internals were read from: a different version fails loudly instead of testing the wrong thing.
// @ts-expect-error untyped internal module (JS with JSDoc only)
import * as formUtils from '../../../node_modules/@sveltejs/kit/src/runtime/form-utils.js';
import kitPackage from '../../../node_modules/@sveltejs/kit/package.json';

const WRITTEN_FOR = '3.0.1';

if (kitPackage.version !== WRITTEN_FOR) {
	throw new Error(
		`kit-form.ts was written for @sveltejs/kit ${WRITTEN_FOR} but ${kitPackage.version} is installed. ` +
			'Re-check form-utils.js (create_field_proxy, convert_formdata, handle_input in ' +
			'client/remote-functions/form.svelte.js) and update WRITTEN_FOR.'
	);
}

export interface FieldContext {
	form_id: string;
	get: () => Record<string, unknown>;
	set: (path: (string | number)[], value: unknown) => void;
	get_issues: () => Record<string, unknown>;
	get_touched: () => Record<string, boolean>;
	get_dirty: () => Record<string, boolean>;
}

/** The `fields` object of a remote form with this id (field.as(...) yields real attribute names). */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export const createFieldProxy = (context: FieldContext): any =>
	formUtils.create_field_proxy(context);

/** FormData -> the object the server-side schema sees. */
export const convertFormData = (formId: string, data: FormData): Record<string, unknown> =>
	formUtils.convert_formdata(formId, data);

export function emptyContext(formId: string): FieldContext {
	return {
		form_id: formId,
		get: () => ({}),
		set: () => {},
		get_issues: () => ({}),
		get_touched: () => ({}),
		get_dirty: () => ({})
	};
}
