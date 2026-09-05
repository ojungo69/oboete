import { type MemoryFilters, resolveProject } from "@codemem/core";

function cleanProject(value: string | null | undefined): string | null {
	const trimmed = value?.trim();
	return trimmed || null;
}

export function resolveDefaultProject(): string | null {
	return cleanProject(process.env.CODEMEM_PROJECT) ?? resolveProject(process.cwd());
}

export function resolveWriteProject(input: {
	project?: string | null;
	envProject?: string | null;
}): string | null {
	return cleanProject(input.project) ?? cleanProject(input.envProject) ?? null;
}

export function buildFilters(
	raw: Record<string, unknown>,
	defaultProject = resolveDefaultProject(),
): MemoryFilters | undefined {
	const filters: MemoryFilters = {};
	let hasAny = false;

	const explicitProject = typeof raw.project === "string" ? cleanProject(raw.project) : undefined;
	const resolvedProject = explicitProject || cleanProject(defaultProject) || undefined;
	if (resolvedProject) {
		filters.project = resolvedProject;
		hasAny = true;
	}

	if (raw.kind !== undefined && raw.kind !== null) {
		filters.kind = String(raw.kind);
		hasAny = true;
	}

	return hasAny ? filters : undefined;
}
