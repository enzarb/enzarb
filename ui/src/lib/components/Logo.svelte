<script lang="ts">
	// `mark` is the simplified flat mark plus the name set in text; `wordmark` swaps the text for the
	// logo wordmark (static/brand/enzarb-wordmark*.svg, inlined so its colours follow the theme).
	// The flat mark carries no gradients or ids, so any number of instances can share a page.
	let { variant = 'mark', size = 22 }: { variant?: 'mark' | 'wordmark'; size?: number } = $props();
</script>

<a class="ez-logo" href="/" aria-label="enzarb home">
	<svg class="ez-logo__mark" width={size} height={size} viewBox="0 0 64 64" aria-hidden="true">
		<rect width="64" height="64" rx="14" fill="#0b0f17" />
		<polygon points="12,12 52,12 52,21 21,21 21,27 36,27 36,35 21,35 21,52 12,52" fill="#00d8f6" />
		<polygon points="52,24 52,33 38,43 52,43 52,52 25,52 25,44 44,30" fill="#10b981" />
	</svg>
	{#if variant === 'wordmark'}
		<svg
			class="ez-logo__word"
			height={Math.round(size * 0.64)}
			viewBox="280 750 650 74"
			aria-hidden="true"
		>
			<g fill="var(--ez-logo-ink)">
				<path
					d="M 288,760 L 346,760 L 346,772 L 303,772 L 288,785 Z M 288,789 L 303,777 L 303,781 L 335,781 L 335,792 L 303,792 L 303,801 L 347,801 L 347,813 L 288,813 Z"
				/>
				<path
					d="M 358,760 L 372,760 L 407,796 L 407,760 L 421,760 L 421,813 L 407,813 L 372,777 L 372,813 L 358,813 Z"
				/>
				<path
					d="M 432,760 L 491,760 L 491,771 L 452,801 L 491,801 L 491,813 L 431,813 L 431,802 L 470,772 L 432,772 Z"
				/>
				<path
					d="M 498,813 L 527,760 L 543,760 L 572,813 L 556,813 L 550,801 L 520,801 L 514,813 Z M 525,790 L 545,790 L 535,771 Z"
				/>
				<path
					d="M 580,760 L 626,760 C 639,760 647,766 647,777 C 647,786 640,792 630,793 L 648,813 L 631,813 L 615,794 L 595,794 L 595,813 L 580,813 Z M 595,771 L 595,783 L 623,783 C 629,783 632,781 632,777 C 632,773 629,771 623,771 Z"
				/>
				<path
					d="M 659,760 L 706,760 C 718,760 725,765 725,773 C 725,779 721,783 715,785 C 723,787 728,792 728,799 C 728,808 720,813 706,813 L 659,813 Z M 674,771 L 674,781 L 702,781 C 707,781 710,779 710,776 C 710,773 707,771 702,771 Z M 674,791 L 674,802 L 704,802 C 709,802 713,800 713,796 C 713,793 709,791 704,791 Z"
				/>
			</g>
			<g fill="var(--ez-logo-dev)">
				<circle cx="742" cy="806" r="7" />
				<path
					d="M 792,760 L 806,760 L 806,813 L 793,813 L 793,807 C 789,812 782,814 775,814 C 762,814 753,805 753,792 C 753,779 762,770 775,770 C 782,770 788,772 792,777 Z M 780,780 C 772,780 767,785 767,792 C 767,799 772,804 780,804 C 787,804 792,799 792,792 C 792,785 787,780 780,780 Z"
				/>
				<path
					d="M 815,792 C 815,779 825,770 839,770 C 854,770 863,779 863,793 L 863,796 L 829,796 C 830,802 835,805 842,805 C 847,805 852,803 855,800 L 861,807 C 856,812 849,814 840,814 C 825,814 815,805 815,792 Z M 850,788 C 849,782 845,779 839,779 C 833,779 829,782 829,788 Z"
				/>
				<path d="M 866,771 L 881,771 L 892,800 L 904,771 L 918,771 L 899,813 L 885,813 Z" />
			</g>
		</svg>
	{:else}
		<span>enzarb</span>
	{/if}
</a>
