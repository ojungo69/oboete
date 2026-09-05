const ALLOWED_MEMORY_KINDS = new Set([
	"discovery",
	"change",
	"feature",
	"bugfix",
	"refactor",
	"decision",
	"exploration",
	"session_summary",
]);

export function validateMemoryKind(kind: string): string {
	const normalized = kind.trim().toLowerCase();
	if (!ALLOWED_MEMORY_KINDS.has(normalized)) {
		throw new Error(
			`Invalid memory kind "${kind}". Allowed: ${[...ALLOWED_MEMORY_KINDS].join(", ")}`,
		);
	}
	return normalized;
}
