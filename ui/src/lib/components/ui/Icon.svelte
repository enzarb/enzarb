<script lang="ts" module>
	/** Stroke paths on a 24x24 grid (Lucide-style, drawn in-house). */
	const paths = {
		check: 'M5 12.5l4.5 4.5L19 7.5',
		x: 'M6 6l12 12M18 6L6 18',
		circle: 'M12 21a9 9 0 100-18 9 9 0 000 18z',
		progress: 'M12 3a9 9 0 100 18V3z',
		dot: 'M12 12.01v-.02',
		info: 'M12 21a9 9 0 100-18 9 9 0 000 18zM12 11v5M12 8h.01',
		alert: 'M12 3.5l9.5 16.5h-19L12 3.5zM12 10v4.5M12 17.5h.01',
		'alert-circle': 'M12 21a9 9 0 100-18 9 9 0 000 18zM12 7.5v5M12 16h.01',
		'chevron-down': 'M6 9.5l6 6 6-6',
		plus: 'M12 5v14M5 12h14',
		sun: 'M12 16.5a4.5 4.5 0 100-9 4.5 4.5 0 000 9zM12 2.5v2M12 19.5v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2.5 12h2M19.5 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4',
		moon: 'M20 14.5A8.5 8.5 0 019.5 4 8.5 8.5 0 1020 14.5z',
		monitor: 'M3.5 4.5h17v11h-17zM8.5 20h7M12 15.5V20',
		external:
			'M14 4.5h5.5V10M19.5 4.5L11 13M17.5 14v4.5a1 1 0 01-1 1h-10a1 1 0 01-1-1v-10a1 1 0 011-1H11',
		inbox:
			'M3.5 13.5l2.4-8a1 1 0 011-.7h10.2a1 1 0 011 .7l2.4 8M3.5 13.5v5a1 1 0 001 1h15a1 1 0 001-1v-5M3.5 13.5h5l1 2.5h5l1-2.5h5',
		logout: 'M14 4.5h4.5a1 1 0 011 1v13a1 1 0 01-1 1H14M10 8l-4 4 4 4M6 12h10',
		trash: 'M4.5 7h15M9.5 7V4.5h5V7M6.5 7l.8 12a1 1 0 001 .9h7.4a1 1 0 001-.9l.8-12M10 11v5M14 11v5'
	} as const;

	export type IconName = keyof typeof paths;
</script>

<script lang="ts">
	/** Decorative inline icon (aria-hidden); size follows the surrounding font size. */
	let { name }: { name: IconName } = $props();
</script>

<svg class="ez-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
	<path d={paths[name]} />
</svg>
