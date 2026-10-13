export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';
export type ButtonSize = 'md' | 'sm';

/** Class list shared by Button (a <button>) and LinkButton (an <a>), so both look identical. */
export function buttonClass(
	variant: ButtonVariant,
	size: ButtonSize,
	icon: boolean,
	loading: boolean
): string {
	return [
		'ez-btn',
		`ez-btn--${variant}`,
		size === 'sm' && 'ez-btn--sm',
		icon && 'ez-btn--icon',
		loading && 'is-loading'
	]
		.filter(Boolean)
		.join(' ');
}
