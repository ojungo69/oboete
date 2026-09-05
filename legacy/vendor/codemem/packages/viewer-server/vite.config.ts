import { resolve } from "node:path";
import { defineConfig } from "vitest/config";

import licenseNoticePlugin from "../../scripts/license-notice-plugin.mjs";

// Library mode — SSR/Node target.
// Externalizes @codemem/core, hono, and node: built-ins.
export default defineConfig({
	resolve: {
		alias: {
			"@codemem/core": resolve(import.meta.dirname, "../core/src/index.ts"),
		},
		conditions: ["source"],
	},
	build: {
		lib: {
			entry: resolve(import.meta.dirname, "src/index.ts"),
			formats: ["es"],
			fileName: "index",
		},
		rollupOptions: {
			external: [/^@codemem\//, /^node:/, /^hono/, /^better-sqlite3/],
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
		name: "viewer-server",
	},
});
