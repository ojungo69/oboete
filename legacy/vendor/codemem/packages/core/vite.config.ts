import { resolve } from "node:path";
import { defineConfig } from "vitest/config";

import licenseNoticePlugin from "../../scripts/license-notice-plugin.mjs";

export default defineConfig({
	build: {
		lib: {
			entry: {
				index: resolve(import.meta.dirname, "src/index.ts"),
			},
			formats: ["es"],
			fileName: (_format, entryName) => `${entryName}.js`,
		},
		rollupOptions: {
			external: [
				"better-sqlite3",
				"sqlite-vec",
				"@xenova/transformers",
				"drizzle-orm",
				/^drizzle-orm\//,
				/^node:/,
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
		name: "core",
	},
});
