export const toErrorMessage = (e: unknown, fallback: string) => (e instanceof Error ? e.message : fallback);
