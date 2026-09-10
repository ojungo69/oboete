# macOS platform check runbook (T040 / SC-006)

Target: an Apple Silicon Mac (M1 iMac, stock macOS) reached over remote desktop. Goal: run the
same gate that Linux/WSL pass, on real macOS, and bring the receipts back. No agent CLI, no real
model and no account is needed; everything runs in an isolated home under `/tmp`.

## 1. Prerequisites on the Mac (one time, ~10 minutes)

```bash
xcode-select --install || true          # git; skip if already present
curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash
export NVM_DIR="$HOME/.nvm"; . "$NVM_DIR/nvm.sh"
nvm install 24.16.0 && nvm install 22.16.0 && nvm use 24.16.0
node -v && npm -v && git --version && sw_vers && uname -m
```

## 2. Get the exact source

Clone the public repository at the commit under test: the head of the 009 pull request (branch
`009-memory-core`), or the exact commit if the Linux session names one.

```bash
mkdir -p ~/oboete-check && cd ~/oboete-check
git clone --branch 009-memory-core https://github.com/ojungo69/free-mem.git && cd free-mem
git rev-parse HEAD | tee ~/oboete-check/commit.txt   # record which commit ran
npm ci
```

## 3. Run the gate exactly as `package.json` does, sequentially

```bash
R=~/oboete-check/receipts; mkdir -p "$R"
sw_vers > "$R/macos-version.txt"; uname -a >> "$R/macos-version.txt"
npm run typecheck > "$R/typecheck.log" 2>&1; echo "typecheck $?" >> "$R/summary.txt"
npm run lint      > "$R/lint.log"      2>&1; echo "lint $?"      >> "$R/summary.txt"
npm run build     > "$R/build.log"     2>&1; echo "build $?"     >> "$R/summary.txt"
for v in 24.16.0 22.16.0; do
  nvm use "$v" >/dev/null; node -v >> "$R/summary.txt"
  node --test --enable-source-maps --test-reporter=tap --test-reporter-destination="$R/unit-$v.tap" \
    'build/test/unit/**/*.test.mjs' 'build/test/migrations/**/*.test.mjs' 'scripts/e2e/**/*.test.mjs' \
    'scripts/dco-check.test.mjs' 'scripts/pack-check.test.mjs' > "$R/unit-$v.log" 2>&1
  echo "unit-$v $?" >> "$R/summary.txt"
  node --test --enable-source-maps --test-concurrency=1 --test-reporter=tap \
    --test-reporter-destination="$R/serial-$v.tap" \
    'build/test/e2e-*.test.mjs' 'build/test/fault-*.test.mjs' 'scripts/quality-debt-record*.test.mjs' \
    > "$R/serial-$v.log" 2>&1
  echo "serial-$v $?" >> "$R/summary.txt"
done
nvm use 24.16.0 >/dev/null
npm run pack-check > "$R/pack.log" 2>&1; echo "pack $?" >> "$R/summary.txt"
```

Keep the phases sequential: overlapping build/pack with the timed hook tests produces the known
load-only seed miss (`capture hit its 300 ms deadline under load`).

## 4. Packed CLI smoke in an isolated home

```bash
npm pack --silent > /dev/null   # writes oboete-<version>.tgz in the repo root from the fresh build
H=$(mktemp -d /tmp/oboete-home.XXXXXX); P=$(mktemp -d /tmp/oboete-pkg.XXXXXX)
tar -xzf oboete-*.tgz -C "$P" && (cd "$P/package" && npm install --omit=dev --ignore-scripts >/dev/null)
OBOETE_HOME="$H" node "$P/package/dist/oboete.mjs" --version   > "$R/packed-version.txt"
OBOETE_HOME="$H" node "$P/package/dist/oboete.mjs" doctor --json > "$R/packed-doctor.json" 2> "$R/packed-doctor.err"
echo "packed-doctor $?" >> "$R/summary.txt"
```

If the packed entry path differs, use the one `scripts/pack-check.mjs` prints.

## 5. Bring the receipts back

```bash
cd ~/oboete-check && tar -czf macos-receipts-$(date +%Y%m%d).tgz receipts
```

Copy the archive to the Linux host into `/var/tmp/oboete-009-20260909.jJ5grc/macos/` (scp, AirDrop
to a phone, or paste `summary.txt` plus the `# pass/# fail` lines of each `.tap` into the chat if
transfer is awkward). The Linux session records the numbers in `quickstart.md` and marks T040's
macOS row as passed, failed, or unavailable with the exact receipt names.

## What counts

- PASS: every phase exit 0, both Node versions, `# fail 0` in all four `.tap` files, packed doctor exit 0.
- Any failure stays recorded as a failure on macOS; it is not reclassified as unavailable. Unavailable
  is only for a check that cannot run at all on this machine (say which command failed to start).
