declare global {
	namespace App {
		interface Error {
			message: string;
		}
		interface Locals {
			session: import('$lib/session').Session | null;
		}
		interface PageData {}
		interface Platform {}
	}
}

export {};
