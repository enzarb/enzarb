/**
 * Subscribe to `/api/events`. `onChange(id)` fires per changed work item; `onChange(null)` fires
 * after a reconnect, when changes may have been missed. Returns the unsubscribe function.
 */
export function watchWorkItems(onChange: (id: number | null) => void): () => void {
	if (typeof EventSource === 'undefined') return () => {};
	const source = new EventSource('/api/events');
	let dropped = false;
	source.onopen = () => {
		if (dropped) onChange(null);
		dropped = false;
	};
	source.onerror = () => {
		dropped = true;
	};
	source.onmessage = (e) => {
		try {
			const { id } = JSON.parse(e.data) as { id?: unknown };
			if (typeof id === 'number') onChange(id);
		} catch {
			/* ignore malformed frames */
		}
	};
	return () => source.close();
}
