import { z } from "zod";

export const filterSchema = {
	kind: z.string().max(512).optional().describe("Filter by memory kind"),
	project: z
		.string()
		.max(512)
		.optional()
		.describe("Filter by project scope (matches sessions.project)"),
};

export const memoryKindSchema = z.enum([
	"discovery",
	"change",
	"feature",
	"bugfix",
	"refactor",
	"decision",
	"exploration",
]);

export const filterNames = Object.keys(filterSchema).toSorted();
