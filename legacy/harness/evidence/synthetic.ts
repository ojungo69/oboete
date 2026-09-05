/**
 * test 用の合成観測記録。**production の経路からは使わない。**
 *
 * 既存の committed raw 16 件はすべて legacy 証拠（manifest を持たない）なので、
 * 現行データだけでは「正しい証拠が本当に real-cli-e2e へ上がる」ことを確かめられない。
 * 常に source-test を返す壊れた実装でも負例と移行結果は全部通ってしまうため、
 * positive control をここで組み立てる。
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { assembleFromFixtures, validateFixture } from "../assemble.ts";
import { digestCapture, digestRaw, NORMALIZATION_VERSION } from "./normalize.ts";
import type { EvidenceRef } from "../schema/capability.ts";

export interface CaptureLine {
  event: string;
  at: string;
  payload: Record<string, unknown>;
}

const AT = "2026-08-12T00:00:00.000Z";
/** SessionStart / UserPromptSubmit / Stop / SessionEnd の 1 run（Claude 形） */
export function lifecycle(session: string, prompt: string): CaptureLine[] {
  const p = (extra: Record<string, unknown>): Record<string, unknown> => ({
    session_id: session,
    cwd: "/w",
    transcript_path: "/w/t.jsonl",
    ...extra,
  });
  return [
    { event: "SessionStart", at: AT, payload: p({ hook_event_name: "SessionStart", source: "startup" }) },
    { event: "UserPromptSubmit", at: AT, payload: p({ hook_event_name: "UserPromptSubmit", prompt_id: prompt, prompt: "hello" }) },
    { event: "Stop", at: AT, payload: p({ hook_event_name: "Stop", prompt_id: prompt, last_assistant_message: "hi" }) },
    { event: "SessionEnd", at: AT, payload: p({ hook_event_name: "SessionEnd", prompt_id: prompt, reason: "other" }) },
  ];
}

/** Codex 形。turn 境界は turn_id の共有で native に識別できる */
export function codexLifecycle(session: string, turn: string): CaptureLine[] {
  const p = (extra: Record<string, unknown>): Record<string, unknown> => ({
    session_id: session,
    cwd: "/w",
    ...extra,
  });
  return [
    { event: "SessionStart", at: AT, payload: p({ hook_event_name: "SessionStart", source: "startup" }) },
    { event: "UserPromptSubmit", at: AT, payload: p({ hook_event_name: "UserPromptSubmit", turn_id: turn, prompt: "hello" }) },
    { event: "Stop", at: AT, payload: p({ hook_event_name: "Stop", turn_id: turn, last_assistant_message: "hi" }) },
  ];
}

/** PreToolUse / PostToolUse の対 */
export function toolRun(session: string): CaptureLine[] {
  const p = (extra: Record<string, unknown>): Record<string, unknown> => ({
    session_id: session,
    cwd: "/w",
    tool_name: "Bash",
    tool_use_id: "t1",
    ...extra,
  });
  return [
    { event: "PreToolUse", at: AT, payload: p({ hook_event_name: "PreToolUse" }) },
    { event: "PostToolUse", at: AT, payload: p({ hook_event_name: "PostToolUse" }) },
  ];
}

/** 親 hook に届く SubagentStop */
export function subagentRun(session: string): CaptureLine[] {
  return [
    ...lifecycle(session, "p-sub"),
    {
      event: "SubagentStop",
      at: AT,
      payload: {
        hook_event_name: "SubagentStop",
        session_id: session,
        agent_id: "a1",
        agent_type: "general-purpose",
        agent_transcript_path: "/w/sub.jsonl",
      },
    },
  ];
}

/** 記録の時刻を差し替える。1 つの fixture が複数の run を束ねる形を組むのに使う */
export function atTime(lines: CaptureLine[], at: string): CaptureLine[] {
  return lines.map((l) => ({ ...l, at }));
}

export interface PutOptions {
  /** manifest を書くか。書かないと legacy 証拠になり real-cli-e2e の根拠にならない */
  manifest?: boolean;
  cli?: "claude" | "codex";
  cliVersion?: string;
  scenarioId?: string;
  /** manifest の欄を差し替える（照合表を 1 項目ずつ反転する test 用） */
  manifestOverrides?: Record<string, unknown>;
  /** manifest ファイルの生 byte を差し替える（parse 前の照合を見る test 用） */
  manifestBytes?: Uint8Array;
}

/** 観測記録（と任意で manifest）を置き場へ書き、fixture が名指しする ref を返す */
export function putEvidence(
  root: string,
  label: string,
  lines: CaptureLine[],
  opts: PutOptions = {},
): EvidenceRef {
  const bytes = Buffer.from(`${lines.map((l) => JSON.stringify(l)).join("\n")}\n`, "utf8");
  writeFileSync(join(root, `${label}.jsonl`), bytes);
  const ref: EvidenceRef = {
    path: `${label}.jsonl`,
    evidenceHash: digestCapture(bytes),
    captureRawHash: digestRaw(bytes),
    normalizationVersion: NORMALIZATION_VERSION,
  };
  if (!opts.manifest) return ref;

  const manifest = {
    manifestVersion: 1,
    cli: opts.cli ?? "claude",
    cliVersion: opts.cliVersion ?? "1.2.3-test",
    scenarioId: opts.scenarioId ?? "self.test",
    // rig と同じく記録の 1 行目から取る。定数を書くと、検証側が記録ではなく
    // fixture に縛っていても positive control が通ってしまう
    capturedAt: lines[0]?.at ?? AT,
    isolated: true,
    internalRunMarker: true,
    exitStatus: 0,
    recorderErrors: 0,
    capture: `${label}.jsonl`,
    captureRawHash: ref.captureRawHash,
    captureHash: ref.evidenceHash,
    normalizationVersion: NORMALIZATION_VERSION,
    ...(opts.manifestOverrides ?? {}),
  };
  const written = opts.manifestBytes ?? Buffer.from(JSON.stringify(manifest), "utf8");
  writeFileSync(join(root, `${label}.manifest.json`), written);
  return {
    ...ref,
    manifest: `${label}.manifest.json`,
    // manifestHash は **書いた byte** から取る。opts.manifestBytes で壊した場合も
    // hash 自体は一致させ、「壊れた manifest を parse する前に落とすか」を見る
    manifestHash: digestRaw(written),
  };
}

/** 最小の CaptureFixture。test ごとに必要な欄だけ上書きする */
export function fixtureBase(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    fixtureId: "claude/synthetic",
    cli: "claude",
    nativeVersion: "1.2.3-test",
    capturedAt: AT,
    scenario: "synthetic",
    scenarioId: "self.test",
    observedEvents: [],
    toolFailurePhasesObserved: [],
    limitations: [],
    limitationCodes: [],
    rig: { isolated: true, internalRunMarker: true },
    ...overrides,
  };
}

/** test 用の空の証拠置き場 */
export { newRoot } from "./scratch.ts";

/**
 * schema と手書き検証も通してから組み立てる。fixture だけ先に変える経路を作らない。
 * 呼び手ごとに書くと、片方だけ validateFixture を飛ばした test が混ざる。
 */
export const assembleWithRoot = (fixtures: Record<string, unknown>[], root: string) =>
  assembleFromFixtures(
    fixtures.map((f, i) => validateFixture(JSON.parse(JSON.stringify(f)), `f${i}.json`)),
    { evidenceRoot: root },
  );
