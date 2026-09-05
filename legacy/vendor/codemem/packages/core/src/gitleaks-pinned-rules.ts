import { createHash } from "node:crypto";

const MAX_RULES = 100;
const MAX_PATTERN_LENGTH = 512;

export const GITLEAKS_PIN = {
	version: "8.30.1",
	configUrl: "https://raw.githubusercontent.com/gitleaks/gitleaks/v8.30.1/config/gitleaks.toml",
	configSha256: "e163e53b9e7e8a8511e77271e2b323ed057759542a6d988258afe3a1fa329caf",
	subsetContractVersion: 1,
} as const;

export type GitleaksRuleSource = {
	id: string;
	regex: string;
	entropy?: number;
	secretGroup?: number;
};

export type ConvertedGitleaksRule = {
	kind: string;
	pattern: RegExp;
	minEntropy?: number;
	redactGroup?: number;
	origin: string;
};

const PINNED_SUBSET: readonly GitleaksRuleSource[] = [
	{
		id: "age-secret-key",
		regex: "AGE-SECRET-KEY-1[QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L]{58}",
	},
	{
		id: "artifactory-api-key",
		regex: String.raw`\bAKCp[A-Za-z0-9]{69}\b`,
		entropy: 4.5,
	},
	{
		id: "sentry-user-token",
		regex: String.raw`\b(sntryu_[a-f0-9]{64})(?:[\x60'"\s;]|\\[nr]|$)`,
		entropy: 3.5,
	},
	{
		id: "shippo-api-token",
		regex: String.raw`\b(shippo_(?:live|test)_[a-fA-F0-9]{40})(?:[\x60'"\s;]|\\[nr]|$)`,
		entropy: 2,
	},
	{
		id: "shopify-access-token",
		regex: "shpat_[a-fA-F0-9]{32}",
		entropy: 2,
	},
	{
		id: "sonar-api-token",
		regex: String.raw`(?i)[\w.-]{0,50}?(?:sonar[_.-]?(login|token))(?:[ \t\w.-]{0,20})[\s'"]{0,3}(?:=|>|:{1,3}=|\|\||:|=>|\?=|,)[\x60'"\s=]{0,5}((?:squ_|sqp_|sqa_)?[a-z0-9=_\-]{40})(?:[\x60'"\s;]|\\[nr]|$)`,
		secretGroup: 2,
	},
];

export const GITLEAKS_PINNED_RULE_IDS = PINNED_SUBSET.map((rule) => rule.id);

export function countRegExpCaptureGroups(re: RegExp): number {
	// The input is an already-compiled, bounded rule; dynamic compilation is required to count groups.
	const match = new RegExp(`(?:${re.source})|`, re.flags.replace(/[gy]/g, "")).exec(""); // nosemgrep
	return Math.max(0, (match?.length ?? 1) - 1);
}

export function convertGitleaksRules(
	sources: readonly GitleaksRuleSource[],
): ConvertedGitleaksRule[] {
	if (sources.length > MAX_RULES) throw new Error("gitleaks subset exceeds 100 rules");
	const seen = new Set<string>();
	return sources.map((source) => {
		if (!source.id || seen.has(source.id))
			throw new Error("gitleaks rule id is missing or duplicate");
		seen.add(source.id);
		if (!source.regex || source.regex.length > MAX_PATTERN_LENGTH) {
			throw new Error(`gitleaks rule ${source.id} has an invalid pattern length`);
		}
		let patternSource = source.regex;
		let flags = "g";
		if (patternSource.startsWith("(?i)")) {
			patternSource = patternSource.slice(4);
			flags = "gi";
		}
		const unsupportedGroups = patternSource.replaceAll("(?:", "");
		if (
			unsupportedGroups.includes("(?") ||
			patternSource.includes("[[:") ||
			patternSource.includes(String.raw`\z`) ||
			patternSource.includes(String.raw`\A`) ||
			patternSource.includes(String.raw`\C`) ||
			/\\[1-9]/.test(patternSource)
		) {
			throw new Error(`gitleaks rule ${source.id} uses unsupported regex syntax`);
		}
		if (source.entropy !== undefined && (!Number.isFinite(source.entropy) || source.entropy < 0)) {
			throw new Error(`gitleaks rule ${source.id} has invalid entropy`);
		}
		// Release-pinned patterns are length- and syntax-checked above.
		const pattern = new RegExp(patternSource, flags); // nosemgrep
		if (
			source.secretGroup !== undefined &&
			(!Number.isInteger(source.secretGroup) ||
				source.secretGroup < 1 ||
				source.secretGroup > countRegExpCaptureGroups(pattern))
		) {
			throw new Error(`gitleaks rule ${source.id} has invalid secretGroup`);
		}
		return {
			kind: source.id,
			pattern,
			...(source.entropy === undefined ? {} : { minEntropy: source.entropy }),
			...(source.secretGroup === undefined ? {} : { redactGroup: source.secretGroup }),
			origin: `gitleaks:${GITLEAKS_PIN.version}:${source.id}`,
		};
	});
}

export const MANDATORY_GITLEAKS_RULES = convertGitleaksRules(PINNED_SUBSET);

type FingerprintRule = {
	kind: string;
	pattern: RegExp;
	minEntropy?: number;
	redactGroup?: number;
	origin?: string;
};

export function fingerprintSecretRules(
	rules: readonly FingerprintRule[],
	degraded: boolean,
): string {
	const hash = createHash("sha256")
		.update(
			JSON.stringify({
				gitleaks: {
					version: GITLEAKS_PIN.version,
					configSha256: GITLEAKS_PIN.configSha256,
					subsetContractVersion: GITLEAKS_PIN.subsetContractVersion,
				},
				rules: rules.map((rule) => ({
					origin: rule.origin ?? "codemem",
					kind: rule.kind,
					source: rule.pattern.source,
					flags: rule.pattern.flags,
					minEntropy: rule.minEntropy ?? null,
					redactGroup: rule.redactGroup ?? null,
				})),
			}),
		)
		.digest("hex");
	return degraded ? `${hash}:degraded` : hash;
}
