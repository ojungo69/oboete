// #90 が閉じるまでの機械的な歯止め。
//
// real-cli-e2e への昇格経路は実装され test も通っているが、観測記録は測定対象 CLI と同じ
// UID で書けるので「CLI が自分で作った記録」を最高位証跡として公開できてしまう
// （harness/matrix/README.md「記録の取得側に残っている限界」）。文書は経路を閉じない。
//
// そこで、直せるまでは **成果物の側**を止める。この test は #90 を閉じたときに削除する。
import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import test from "node:test";
import { readIJsonFile } from "../schema/jcs.ts";

const CLIS = ["claude", "codex"] as const;

test("no committed fixture names a manifest while the recorder is forgeable (#90)", () => {
  // 名前の規約ではなく**昇格の入力**で見る。`.manifest.json` という綴りは verification が
  // 要求していないので、suffix だけを見る検査は別名の manifest を素通しする
  for (const cli of CLIS) {
    const dir = new URL(`../fixtures/${cli}/`, import.meta.url);
    const names = readdirSync(dir).filter((n) => n.endsWith(".json"));
    // 件数も主張する。0 件だと内側の assert が 1 度も呼ばれず、歯止めが空振りしたまま緑になる
    assert.ok(names.length > 0, `${cli}: 検査対象の fixture が 1 件も無い`);
    for (const name of names) {
      const fixture = readIJsonFile<{ evidence?: Array<{ manifest?: unknown }> }>(new URL(name, dir));
      for (const ref of fixture.evidence ?? []) {
        assert.equal(ref.manifest, undefined, `${cli}/${name}: #90 が閉じるまで manifest は名指ししない`);
      }
    }
  }
});

test("no rig-written manifest is committed while the recorder is forgeable (#90)", () => {
  for (const cli of CLIS) {
    const dir = new URL(`../fixtures/${cli}/raw/`, import.meta.url);
    const manifests = readdirSync(dir).filter((n) => n.endsWith(".manifest.json"));
    assert.deepEqual(manifests, [], `${cli}: #90 が閉じるまで manifest は commit しない`);
  }
});

test("no shipped matrix cell claims real-cli-e2e while the recorder is forgeable (#90)", () => {
  // 綴りではなく**復号した値**で見る。`"real-cli-\u0065\u0032e"` は JSON として同じ文字列で、
  // byte 列を探す検査だけを素通りする
  const claims = (value: unknown): boolean =>
    typeof value === "string"
      ? value === "real-cli-e2e"
      : Array.isArray(value)
        ? value.some(claims)
        : typeof value === "object" && value !== null
          ? Object.values(value).some(claims)
          : false;
  for (const cli of CLIS) {
    assert.ok(
      !claims(readIJsonFile(new URL(`../matrix/${cli}.json`, import.meta.url))),
      `${cli}: #90 が閉じるまで real-cli-e2e は出荷しない`,
    );
  }
});
