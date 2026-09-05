import { resolve } from "node:path";
import { defineConfig } from "vitest/config";

import licenseNoticePlugin from "../../scripts/license-notice-plugin.mjs";

export default defineConfig({
	resolve: {
		alias: {
			"@codemem/core": resolve(import.meta.dirname, "../core/src/index.ts"),
		},
		conditions: ["source"],
	},
	build: {
		lib: {
			entry: {
				index: "src/index.ts",
				stdio: "src/stdio.ts",
			},
			formats: ["es"],
			fileName: (_format, entryName) => `${entryName}.js`,
		},
		rollupOptions: {
			external: [
				/^@codemem\//,
				/^node:/,
				/^@modelcontextprotocol\//,
				/^express(?:\/.*)?$/,
				"zod",
				"better-sqlite3",
			],
			plugins: [
				licenseNoticePlugin({
					outFile: resolve(import.meta.dirname, "dist/THIRD_PARTY_NOTICES.md"),
				}),
			],
		},
		outDir: "dist",
		sourcemap: true,
		emptyOutDir: true,
	},
	test: {
		name: "mcp-server",
	},
});
