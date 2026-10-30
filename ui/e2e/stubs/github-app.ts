// Stand-in for src/lib/server/github-app.ts in the e2e web server: installation tokens without a
// GitHub App. Everything else is the real module.
export * from '../../src/lib/server/github-app.ts';

export const token = async (): Promise<string | null> => 'e2e-token';
export const invalidateToken = (): void => {};
