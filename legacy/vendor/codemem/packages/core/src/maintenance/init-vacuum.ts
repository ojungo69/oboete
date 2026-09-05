/* Database init + vacuum — bootstrap schema, run relink, vacuum on demand.
 */

import { statSync } from "node:fs";
import type { Database } from "../db.js";

export function vacuumDatabaseWithDb(
	db: Database,
	path = db.name,
): { path: string; sizeBytes: number } {
	db.exec("VACUUM");
	return { path, sizeBytes: statSync(path).size };
}
