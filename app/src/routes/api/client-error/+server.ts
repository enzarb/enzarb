import type { RequestHandler } from './$types';
import { sql } from '$lib/db';

export const POST: RequestHandler = async ({ request, locals }) => {
	let body: { message?: string; stack?: string; context?: Record<string, unknown> };
	try {
		body = await request.json();
	} catch {
		return new Response(null, { status: 400 });
	}
	const message = String(body.message ?? 'Unknown client error').slice(0, 2000);
	const stack = body.stack ? String(body.stack).slice(0, 8000) : null;
	const context = body.context ?? {};
	const userId = (locals as any).session?.userId ?? null;
	try {
		await sql`
			INSERT INTO error_logs (scope, level, message, stack, context, user_id)
			VALUES ('client', 'error', ${message}, ${stack}, ${JSON.stringify(context)}, ${userId})
		`;
	} catch {
		// Best-effort
	}
	return new Response(null, { status: 204 });
};
