import type { Session, SessionUser } from '#lib/server/auth.ts';

declare global {
	namespace App {
		interface Locals {
			user: SessionUser | null;
			session: Session | null;
		}
	}
}

export {};
