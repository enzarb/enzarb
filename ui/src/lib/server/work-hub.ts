import { getSql } from './db.ts';
import { WorkEventHub } from './work-events.ts';

let hub: WorkEventHub | undefined;

/** Process-wide hub (one LISTEN connection per server process). */
export function getWorkEventHub(): WorkEventHub {
	return (hub ??= new WorkEventHub(getSql()));
}
