import license from "rollup-plugin-license";

const EMPTY_NOTICE = "No third-party code is bundled in this artifact.";

function compareDependencies(left, right) {
	const leftKey = `${left.name ?? ""}\0${left.version ?? ""}`;
	const rightKey = `${right.name ?? ""}\0${right.version ?? ""}`;
	return leftKey < rightKey ? -1 : leftKey > rightKey ? 1 : 0;
}

function formatNotice(dependencies) {
	const lines = ["# Third-party notices", "", "Generated from the bundler module graph. Do not edit.", ""];
	const sorted = [...dependencies].sort(compareDependencies);

	if (sorted.length === 0) return `${lines.join("\n")}${EMPTY_NOTICE}\n`;

	for (const dependency of sorted) {
		const name = dependency.name ?? "unknown";
		const version = dependency.version ?? "unknown";
		const declaredLicense = dependency.license ?? "unknown";
		const repository =
			typeof dependency.repository === "string" ? dependency.repository : dependency.repository?.url;
		const licenseText = dependency.licenseText
			? dependency.licenseText.trim()
			: dependency.license
				? `Upstream package does not include a license file. package.json declares \`${dependency.license}\`.`
				: "Upstream package does not include a license file or a package.json license declaration.";

		lines.push(
			"<!-- codemem:dependency -->",
			`## ${name}@${version}`,
			"",
			`- Name: \`${name}\``,
			`- Version: \`${version}\``,
			`- License: \`${declaredLicense}\``,
		);
		if (dependency.author) lines.push(`- Author: ${dependency.author.text()}`);
		if (repository) lines.push(`- Repository: ${repository}`);
		lines.push(
			"",
			"<!-- codemem:license-text -->",
			"### License text",
			"",
			licenseText,
			"<!-- codemem:end-license-text -->",
		);
		if (dependency.noticeText) {
			lines.push("", "### Notice text", "", dependency.noticeText.trim());
		}
		lines.push("", "---", "");
	}

	return `${lines.join("\n").trimEnd()}\n`;
}

export default function licenseNoticePlugin({ outFile }) {
	return license({
		thirdParty: {
			includePrivate: false,
			// 既定の dedupe 鍵は package 名だけなので、同名・異 version が bundle されると 1 件しか
			// notice へ出ない。落ちた方の license 本文と version が黙って消えるため、鍵に version を
			// 含める（harness/notice-inclusion-check.mjs の baseline も `name@version` で持つ）。
			multipleVersions: true,
			output: {
				file: outFile,
				template: formatNotice,
			},
		},
	});
}
