export const E2E_PORT = 5273;
export const E2E_ORIGIN = `http://localhost:${E2E_PORT}`;
export const E2E_USER = 'e2e-user';
export const E2E_PROJECT = 'e2e-project';
export const STORAGE_STATE = 'test-results/.auth/state.json';

/** Dummy values: nothing here talks to GitHub or a realtime service. */
export const SERVER_ENV = {
	ENZARB_SESSION_JWT_SECRET: 'e2e-session-jwt-secret-not-for-production',
	ENZARB_REALTIME_JWT_SECRET: 'e2e-realtime-jwt-secret-not-for-production',
	GITHUB_OAUTH_CLIENT_ID: 'e2e-client-id',
	GITHUB_OAUTH_CLIENT_SECRET: 'e2e-client-secret',
	ENZARB_PUBLIC_URL: E2E_ORIGIN
};

/** What the fake agent says; the test asserts on it. */
export const AGENT_REPLY = 'Hello from the fake agent.';
