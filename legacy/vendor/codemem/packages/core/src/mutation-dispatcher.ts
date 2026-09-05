import { createHash, randomUUID } from "node:crypto";
import type { WriterActor } from "./writer-actor.js";

export type MutationReceipt = {
	receiptId: string;
	idempotencyKey: string;
	payloadHash: string;
	method: string;
	status: "committed" | "conflict";
	result: Record<string, unknown>;
};

export class MutationConflictError extends Error {
	readonly code = "idempotency_conflict";
	readonly receipt: MutationReceipt;

	constructor(
		receipt: MutationReceipt,
		message = "Idempotency key already exists with a different payload.",
	) {
		super(message);
		this.name = "MutationConflictError";
		this.receipt = receipt;
	}
}

const MUTATION_RECEIPT_DDL = `
CREATE TABLE IF NOT EXISTS mutation_receipts (
	receipt_id TEXT PRIMARY KEY NOT NULL,
	idempotency_key TEXT NOT NULL,
	payload_hash TEXT NOT NULL,
	method TEXT NOT NULL,
	status TEXT NOT NULL,
	result_json TEXT NOT NULL,
	created_at TEXT NOT NULL,
	UNIQUE(method, idempotency_key)
);
CREATE TABLE IF NOT EXISTS mutation_quarantine (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	idempotency_key TEXT NOT NULL,
	payload_hash TEXT NOT NULL,
	existing_hash TEXT NOT NULL,
	method TEXT NOT NULL,
	created_at TEXT NOT NULL
);
`;

type SqlExec = { exec(source: string): unknown };

export function ensureMutationReceiptSchema(db: SqlExec): void {
	db.exec(MUTATION_RECEIPT_DDL);
}

export function canonicalMutationJson(value: unknown): string {
	return JSON.stringify(value, (_key, current: unknown) => {
		if (current == null || typeof current !== "object" || Array.isArray(current)) return current;
		return Object.fromEntries(
			Object.entries(current as Record<string, unknown>).toSorted(([left], [right]) =>
				left < right ? -1 : left > right ? 1 : 0,
			),
		);
	});
}

export function hashMutationPayload(value: unknown): string {
	return createHash("sha256").update(canonicalMutationJson(value), "utf8").digest("hex");
}

type ReceiptRow = {
	receipt_id: string;
	idempotency_key: string;
	payload_hash: string;
	method: string;
	status: string;
	result_json: string;
};

function rowToReceipt(row: ReceiptRow): MutationReceipt {
	return {
		receiptId: row.receipt_id,
		idempotencyKey: row.idempotency_key,
		payloadHash: row.payload_hash,
		method: row.method,
		status: row.status === "conflict" ? "conflict" : "committed",
		result: JSON.parse(row.result_json) as Record<string, unknown>,
	};
}

function quarantineMutationConflict(
	db: WriterActor,
	idempotencyKey: string,
	payloadHash: string,
	existingHash: string,
	method: string,
): void {
	db.prepare(
		`INSERT INTO mutation_quarantine(idempotency_key, payload_hash, existing_hash, method, created_at)
		 SELECT ?, ?, ?, ?, ?
		 WHERE NOT EXISTS (
			SELECT 1 FROM mutation_quarantine
			WHERE idempotency_key = ? AND payload_hash = ? AND existing_hash = ? AND method = ?
		 )`,
	).run(
		idempotencyKey,
		payloadHash,
		existingHash,
		method,
		new Date().toISOString(),
		idempotencyKey,
		payloadHash,
		existingHash,
		method,
	);
}

export function dispatchClassA(
	db: WriterActor,
	input: {
		method: string;
		idempotencyKey: string;
		payload: unknown;
		apply: () => Record<string, unknown>;
		/** Runs inside the transaction before an existing key becomes a replay or conflict. */
		reconcileExisting?: (
			existing: MutationReceipt,
			incomingPayloadHash: string,
		) => MutationReceipt | null;
	},
): MutationReceipt {
	if (
		typeof input.idempotencyKey !== "string" ||
		input.idempotencyKey.length === 0 ||
		Buffer.byteLength(input.idempotencyKey, "utf8") > 256
	) {
		throw new Error("idempotencyKey is required.");
	}
	const payloadHash = hashMutationPayload(input.payload);
	const outcome = db.transaction(() => {
		const existing = db
			.prepare(
				"SELECT receipt_id, idempotency_key, payload_hash, method, status, result_json FROM mutation_receipts WHERE method = ? AND idempotency_key = ?",
			)
			.get(input.method, input.idempotencyKey) as ReceiptRow | undefined;
		if (existing) {
			if (input.reconcileExisting) {
				const reconciled = input.reconcileExisting(rowToReceipt(existing), payloadHash);
				if (reconciled && existing.payload_hash !== payloadHash) {
					quarantineMutationConflict(
						db,
						input.idempotencyKey,
						payloadHash,
						existing.payload_hash,
						input.method,
					);
				}
				if (reconciled) return { kind: "ok" as const, receipt: reconciled };
			}
			if (existing.payload_hash === payloadHash) {
				return { kind: "ok" as const, receipt: rowToReceipt(existing) };
			}
			quarantineMutationConflict(
				db,
				input.idempotencyKey,
				payloadHash,
				existing.payload_hash,
				input.method,
			);
			return { kind: "conflict" as const, receipt: rowToReceipt(existing) };
		}
		const result = input.apply();
		const receiptId = randomUUID();
		const createdAt = new Date().toISOString();
		db.prepare(
			`INSERT INTO mutation_receipts(receipt_id, idempotency_key, payload_hash, method, status, result_json, created_at)
			 VALUES (?, ?, ?, ?, 'committed', ?, ?)`,
		).run(
			receiptId,
			input.idempotencyKey,
			payloadHash,
			input.method,
			JSON.stringify(result),
			createdAt,
		);
		return {
			kind: "ok" as const,
			receipt: {
				receiptId,
				idempotencyKey: input.idempotencyKey,
				payloadHash,
				method: input.method,
				status: "committed" as const,
				result,
			},
		};
	})();
	if (outcome.kind === "conflict") throw new MutationConflictError(outcome.receipt);
	return outcome.receipt;
}
