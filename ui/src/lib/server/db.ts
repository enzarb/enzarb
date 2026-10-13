import postgres from 'postgres';
import { getConfig } from './config.ts';

export type Sql = postgres.Sql;

let sql: Sql | undefined;

/** Shared connection pool, created on first use. */
export function getSql(): Sql {
	return (sql ??= postgres(getConfig().databaseUrl, { max: 10, onnotice: () => {} }));
}
