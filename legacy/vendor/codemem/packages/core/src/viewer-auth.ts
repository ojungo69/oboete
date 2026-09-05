import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { lstatSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { durableReplaceFile } from "./storage-platform.js";

export const VIEWER_NONCE_TTL_MS = 60_000;
export const VIEWER_SESSION_TTL_MS = 12 * 60 * 60 * 1_000;
export const VIEWER_SESSION_LIMIT = 8;

const TOKEN_PATTERN = /^[A-Za-z0-9_-]{43}$/;

type Session = { expiresAt: number };

function constantTimeEqual(left: string, right: string): boolean {
	const leftDigest = createHmac("sha256", "codemem-viewer-compare").update(left).digest();
	const rightDigest = createHmac("sha256", "codemem-viewer-compare").update(right).digest();
	return timingSafeEqual(leftDigest, rightDigest);
}

function readBearerToken(path: string): string | null {
	let stat: ReturnType<typeof lstatSync>;
	try {
		stat = lstatSync(path);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
		throw error;
	}
	if (!stat.isFile() || stat.isSymbolicLink()) {
		throw new Error("Viewer bearer token must be a regular file.");
	}
	if ((stat.mode & 0o777) !== 0o600) {
		throw new Error("Viewer bearer token permissions must be 0600.");
	}
	if (process.getuid && stat.uid !== process.getuid()) {
		throw new Error("Viewer bearer token must be owned by the daemon user.");
	}
	const token = readFileSync(path, "utf8").trim();
	if (!TOKEN_PATTERN.test(token) || Buffer.from(token, "base64url").length !== 32) {
		throw new Error("Viewer bearer token is malformed.");
	}
	return token;
}

export function readViewerBearerToken(controlDir: string): string | null {
	return readBearerToken(join(controlDir, "token"));
}

function ensureBearerToken(controlDir: string): string {
	const path = join(controlDir, "token");
	const existing = readViewerBearerToken(controlDir);
	if (existing) return existing;
	const token = randomBytes(32).toString("base64url");
	durableReplaceFile(path, `${token}\n`);
	return token;
}

export class ViewerAuthState {
	private readonly bearerToken: string;
	private readonly instance: string;
	private readonly signingKey = randomBytes(32);
	private readonly now: () => number;
	private readonly nonces = new Map<string, number>();
	private readonly sessions = new Map<string, Session>();

	constructor(options: { controlDir: string; instanceId: string; now?: () => number }) {
		this.bearerToken = ensureBearerToken(options.controlDir);
		this.instance = Buffer.from(options.instanceId, "utf8").toString("base64url");
		this.now = options.now ?? Date.now;
	}

	verifyBearer(token: string): boolean {
		return TOKEN_PATTERN.test(token) && constantTimeEqual(token, this.bearerToken);
	}

	issueNonce(): { nonce: string; expiresAt: number } {
		const now = this.now();
		this.prune(now);
		const nonce = randomBytes(32).toString("base64url");
		const expiresAt = now + VIEWER_NONCE_TTL_MS;
		this.nonces.set(nonce, expiresAt);
		return { nonce, expiresAt };
	}

	exchangeNonce(nonce: string): { cookie: string; expiresAt: number } | null {
		const now = this.now();
		const expiresAt = this.nonces.get(nonce);
		this.nonces.delete(nonce);
		this.prune(now);
		if (expiresAt === undefined || expiresAt <= now) return null;

		while (this.sessions.size >= VIEWER_SESSION_LIMIT) {
			const oldest = this.sessions.keys().next().value;
			if (oldest === undefined) break;
			this.sessions.delete(oldest);
		}

		const id = randomBytes(32).toString("base64url");
		const sessionExpiresAt = now + VIEWER_SESSION_TTL_MS;
		this.sessions.set(id, { expiresAt: sessionExpiresAt });
		return { cookie: this.sign(id, sessionExpiresAt), expiresAt: sessionExpiresAt };
	}

	verifySession(cookie: string): boolean {
		const parsed = this.parse(cookie);
		if (!parsed) return false;
		const now = this.now();
		this.prune(now);
		const session = this.sessions.get(parsed.id);
		return session?.expiresAt === parsed.expiresAt && parsed.expiresAt > now;
	}

	logout(cookie: string): boolean {
		const parsed = this.parse(cookie);
		if (!parsed) return false;
		return this.sessions.delete(parsed.id);
	}

	private sign(id: string, expiresAt: number): string {
		const payload = `v1.${id}.${expiresAt}.${this.instance}`;
		const signature = createHmac("sha256", this.signingKey).update(payload).digest("base64url");
		return `${payload}.${signature}`;
	}

	private parse(cookie: string): { id: string; expiresAt: number } | null {
		const parts = cookie.split(".");
		if (parts.length !== 5) return null;
		const [version, id, expiresText, instance, signature] = parts;
		if (
			version !== "v1" ||
			!id ||
			!TOKEN_PATTERN.test(id) ||
			!expiresText ||
			!/^\d+$/.test(expiresText) ||
			instance !== this.instance ||
			!signature
		) {
			return null;
		}
		const expiresAt = Number(expiresText);
		if (!Number.isSafeInteger(expiresAt)) return null;
		const payload = `${version}.${id}.${expiresText}.${instance}`;
		const expected = createHmac("sha256", this.signingKey).update(payload).digest("base64url");
		if (!constantTimeEqual(signature, expected)) return null;
		return { id, expiresAt };
	}

	private prune(now: number): void {
		for (const [nonce, expiresAt] of this.nonces) {
			if (expiresAt <= now) this.nonces.delete(nonce);
		}
		for (const [id, session] of this.sessions) {
			if (session.expiresAt <= now) this.sessions.delete(id);
		}
	}
}
