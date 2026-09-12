# Injection validation snapshot — measured C3 correction

The C2 none-provider 1,051-event replay exceeded the 300 ms ordinary hook bound. An isolated
hook over a backup of that synthetic database took 483.5 ms inside the process, including 36 Git
invocations totaling 153.9 ms. The same verified source root was resolved repeatedly by each
source-specific detector and again for the whole pack. Do not relax the hook budget or source policy.

The existing assembly already obtains one privacy snapshot for all kept references before any
asynchronous detection, then independently recomputes its guard immediately before writing the
injection. Reuse that initial snapshot's source policies, full stored fields and base detector for
all checks within that one pack. The reference must be present in that snapshot; an unknown
reference fails closed. A base-only check uses only the captured base policy. A prepared snapshot
is local to one assembly and is never persisted or shared with another pack or hook.

Keep the final fresh `injectionPrivacyValid` check, the transactional work-selection check, and
Grok's later emission/merge validation. Those calls receive no cached state. A policy, credential,
source label/body/proof, binding, work selection or directory generation that changed after the
initial snapshot cancels the prepared pack. Deferred merging still uses its independent current
validation. Source-specific policies already include home/current-repository rules plus origin
rules; their checks need not repeat the identical text under the narrower base policy.

Verify the actual kept-reference text against the same snapshot whose digest is checked at the
end, including races during async detection and removed/replaced source roots. Record before/after
Git-call and wall-time measurements on the same synthetic workload. Required checks are the
existing work-reader, injection, deferred and Pi privacy/race tests plus the real CLI reproduction.
Worker RSS is a separate measured issue; this change does not claim to resolve it.

## Worker RSS: disable unused library profiling

The C2 uninstrumented replay reached 188.1 MiB. Bounded heap profiles attribute 77.6% of retained
fallback-run samples to Secretlint's global performance marks/measures, rather than source text.
Its installed 13.0.5 API and [versioned upstream README](https://github.com/secretlint/secretlint/blob/v13.0.5/packages/@secretlint/profiler/README.md)
document `secretLintProfiler.setEnabled(false)` for library callers. Set it once in the detector
module before any lint call, in both main processes and detector threads. All rules and revalidation
remain enabled. Declare the already-installed `@secretlint/profiler@13.0.5` directly alongside the
other bundled Secretlint build dependencies, because the engine now imports its shared instance.
The lockfile must retain the same dependency version/integrity. A repeated-detector check must
still redact secrets while retaining zero profiler entries; repeat the uninstrumented replay for RSS.

## Two packs in one Codex response

A reproduced race changed credentials while the second pack was being validated: the earlier
start pack was still printed and had already been marked delivered. Keep its items planned until
the combined response is ready. Pass its planned memory IDs to prompt selection only to suppress
duplicates in that same response; this is not a delivered-memory receipt. After the second await,
freshly validate the earlier pack's work/privacy guard, omit it if stale, then confirm only the
surviving output. Reuse the existing planned-item omission helper for cancellation. Other agent
paths keep their single-pack or deferred delivery protocol.

If the second await throws, cancel the first plan best-effort before returning the original hook
failure. Mark caller-cancelled start packs `omitted/not_delivered` and permit their retry; ordinary
empty/unsupported omissions still count as attempts. A cancelled plan is neither a delivered item
nor proof that the epoch received its start pack.

## Hook cold start: the engine bundle is compiled on every invocation

Merging US6 (`af871c9a`) moved the capture hook's median from 187.3 ms to 222.2 ms, a third of the
head-room under the 300 ms bound. The cost is not in the hook's own work: `dist/oboete.mjs` was a
single-file esbuild bundle, and a hook is a cold Node process, so every invocation paid a full parse
and compile of a file that had just grown by `src/sync/`.

Node's V8 compile cache removes that cost, but a single-file bundle cannot enable it for itself --
Node compiles the entry file before any statement in it runs. `dist/` is therefore two files:
`dist/engine.mjs` is the bundle, and `dist/oboete.mjs` is `src/launcher.mjs` copied verbatim, a
launcher that imports only `node:module`, `node:fs`, `node:os`, `node:path` and `node:url`, enables
the cache,
then imports the engine. It is a real source file rather than a string in the build script so that
the one new piece of hook-path code is covered by the same lint the rest of the tree is, which took
adding it to both `files` lists in `eslint.config.js` -- a `.mjs` under `src/` matched neither. It
imports the engine through its own real path rather than as `./engine.mjs`, because a global install
runs the bin symlink npm creates and `--preserve-symlinks-main` makes `import.meta.url` that link,
under which a relative specifier looks for the engine beside the link and every invocation fails.

Splitting `dist/` splits what "the bundle" means, and every site that names one of the two files has
to pick deliberately. What names the program to run -- the `bin` entry, the hook commands
`oboete setup` writes, the detector worker script, the Pi loader -- names `dist/oboete.mjs`, or the
cache is never enabled where it matters. Existing installs already name that file, so nothing has to
be rewritten. What reports on the build -- the T068 replay record and `scripts/measure-cold-start.mjs` -- names
both files rather than choosing between them. Each prints the file it ran with that file's own size
and, when an `engine.mjs` that is not that same file sits beside it, names that separately with its
own size. Reporting the sibling's bytes under the heading of the file that ran is right for
`dist/oboete.mjs` and wrong for anything else: a `--bundle` naming a single-file build that happens
to share a directory with an unrelated engine -- a baseline copied into `dist/` for a comparison --
would carry the split engine's two megabytes against its own timings, and the receipt would describe
two artefacts as one. The sibling is found through `realpathSync`, because a global install runs a
symlinked `bin` and the engine sits next to the real file, not next to the link; a pre-split build
simply has no second half to report, which keeps the script able to measure one for comparison.
Inside the bundle
`import.meta.url` is now the engine, so `src/setup/setup.ts` composes the launcher path from its
directory and falls back to itself if no launcher is there, since a wired path that does not exist
would make every hook a silent no-op. `process.argv[1]` is not a substitute for `import.meta.url`
here, because it is the test runner when a test imports the engine in-process
(`test/unit/cli.test.ts`, which imports the engine for exactly that reason). The detector worker
does still boot the launcher, since `process.argv[1]` is what names its script: it repeats the two
directory checks and one dynamic import per worker, and it also gets the cache -- the medians below
are whole-hook and include that worker.

The cache directory is `$OBOETE_HOME/cache/compile`, or `~/.oboete/cache/compile`, created with
mode 0700 -- as is the data directory itself when the launcher is what creates it, which happens
whenever a hook runs before `oboete setup` ever has. The rule for finding the home is the one in
`src/paths.ts`: the variable when it names anything, anchored to `homedir()` when it is relative.
That rule is written twice, because importing the engine to read it once is the cost this file
exists to avoid, and a test pins the copies together: `test/unit/launcher.test.ts` spawns the
launcher over every shape the variable takes -- unset, empty, blank, relative, dot-relative, one
with a `..` in it -- and asserts the cache appears where the real `resolveHome` says the home is.
`scripts/measure-cold-start.mjs` carries a third copy, unpinned, which decides only which directory
a record line names.

`$XDG_CACHE_HOME/oboete/compile` was the first choice and it was wrong: `CONSTITUTION.md`
Principle VI puts every path this program writes under one data directory and defers a
per-platform XDG/AppData split to a later milestone, so a cache outside the home survives the
relocation `OBOETE_HOME` exists to perform. The argument for it was that the fault harness gives
every scenario a fresh home, which makes the cache cold per scenario; that is a fact about the
harness, not a licence to write outside the tree, and `scripts/measure-cold-start.mjs` answers it
by running three warm-ups before the thirty measured runs.

Node's own default is the other alternative and it is worse: with no argument `enableCompileCache()` uses
`/tmp/node-compile-cache`, mode 0755 and shared by every user on the machine, and V8 does not
authenticate cache entries, so a writable-by-others cache directory is somewhere to plant bytecode
that the hook will execute.

Creating the directory is not the same as owning it. `mkdirSync(..., { recursive: true, mode })`
leaves an existing directory's mode and owner untouched and follows a symlink, so the launcher
checks what it got -- `lstat`, not a symlink, this uid, the right mode -- and simply does not enable
the cache when the answer is no. A symlink is refused rather than followed, because following one
would let whoever planted it have Node create a directory and write a half-megabyte blob wherever it
points.

The check demands that `compile` be closed to everyone else (`mode & 0o077`), not merely unwritable,
and the reason is where V8 actually reads. Entries live in a versioned directory that **Node**
creates inside `compile`, at 0777 minus the umask -- 0755 on a 022 umask, 0775 on the 002 umask that
RHEL-family user-private-group systems and many CI images use. With no traverse bit for group or
other on `compile`, nothing outside this uid can reach that directory whatever its own mode is.
Inspecting those children instead is the obvious move and it is wrong: it disables the cache from
the second run onward on every 002-umask machine, silently, which is the whole regression back with
no signal.

One directory above is checked, and only for being ours and not a symlink. Recursive `mkdirSync`
follows a link, so a link planted at `cache` would have the launcher create `compile`
inside somebody else's tree and write half a megabyte of bytecode there while every check on
`compile` itself passed -- it would be ours, 0700, a real directory. The parent is therefore checked
*before* `compile` is created, so a refusal leaves nothing behind at all. No mode is required of it:
`compile`'s own traverse bits already decide who can reach the entries.

That last sentence is a test, not a preference. Twice while writing this fix a mode requirement went
onto the parent, and both times every other pin stayed green while the cache went off on
every machine that already had one: the launcher creates that directory itself at 0700, which passes
any mode check at any umask, so only a directory that arrived some other way -- a restored backup,
a `cp -r`, an archive unpacked without modes, all of which give 0777 minus the umask -- ever fails
one. The pin makes that directory at 0775 and asserts the run still fills a cache.

Nothing above that is checked, and the data directory is what "above" now means. Two rounds of this
review tried requiring the ancestors to be unwritable by others, back when the chain ran through
`~/.cache`, and both times it refused real machines. The move under the home shrinks the question
rather than answering it differently: `~/.oboete` is 0700 wherever `ensureDirectories` or the
launcher made it, and a loose one is a problem `oboete doctor` should report -- `memory.db` is in
there -- not one the launcher should answer by silently turning the cache off. A symlink at the
data directory is the user's own arrangement. What write access above buys is the race between the
check and V8's read, and that race is accepted anyway; `homedir()` is not checked either, so
demanding unwritable ancestors would have narrowed it rather than closed it.

The one refusal that stays silent is a `compile` of this user's own left at a loose mode -- a
restored backup, an `rsync` without `-p`, an NFS home. The hook returns to its uncached time with
nothing saying so; `scripts/measure-cold-start.mjs` prints the directory and whether it was non-empty
-- which is a fact about the directory, not about whether the launcher accepted it -- and
`oboete doctor` does not yet have an item for it (issue #218). Refusing is deliberate: correcting
the mode would chmod the target of whatever symlink was planted there. What remains is the ordinary
race between the check and V8's read, which needs write access to the cache directory itself -- or
to a directory above it. Above `compile` is `~/.oboete/cache`, then `~/.oboete`, then `$HOME`. The
outer two settle the same way: an attacker who can write to either can already replace `memory.db`,
`config.toml` and the hook's spool, so bytecode in the compile cache is not the escalation.
`~/.oboete/cache` is the one in between, and it is the whole of the residue. It is checked for
owner and for being a real directory but at any mode, so a group-writable one left behind by a
restored backup lets someone else rename `compile` away after the check has passed. That costs more
than the cache: `enableCompileCache` records a path, nothing stats it again, and V8 opens
`<path>/<version>/<entry>` tens of milliseconds later, when the engine compiles. Whatever stands at
that path by then is what runs -- including a directory of somebody else's holding an entry keyed to
the hash of a bundle they can read out of the published package. Requiring a mode of `cache` is
still not the answer: that is the requirement this review put on twice, and both times it turned the
cache off on every machine that already had a loose one (`test/unit/launcher.test.ts` pins that
shape at 0775). What closes it is a `cache` nobody else can write, which is the 0700 one the
launcher makes when it is absent; a loose one that arrived some other way is a `doctor` item, the
same as a loose data directory, and until `doctor` has that item (issue #218) this is the
accepted residue. A home that cannot hold the directory at
all costs the cache and never the command.

The alternative this does not take is making the hook path its own, smaller entry point. The tree is
already organised around which packages ride inside the engine, so splitting `src/cli.ts` so that
`hook`, `capture` and `inject` compile without the rest would remove the cost rather than remember
it: no cliff on the first run after an upgrade, no cache to grow, and no directory to have to trust.
It is the better answer to the same problem and a much larger change to security-owned bundle
composition; the launcher is what closes the regression now, and the split stays available.

Two more residues, both stated rather than fixed. The launcher imports `node:module` as a namespace
and calls `enableCompileCache` optionally, because a named import is resolved when the module is
linked -- before any statement runs and outside the reach of the surrounding `try` -- so on a Node
older than 22.1 a named import would be a `SyntaxError` and a non-zero exit for every command,
including the hook that is contracted to exit 0 whatever happens. `engines` only warns. And on
Windows there is no uid and no mode, so the checks reduce to "a real directory, not a symlink":
`%USERPROFILE%\.oboete\cache\compile`, or an `OBOETE_HOME` override, is trusted on the strength
of its path alone.

One thing the launcher cannot defend: `NODE_COMPILE_CACHE` in the environment wins. Node enables the
cache at bootstrap from that variable, and a later `enableCompileCache(dir)` returns
`{ status: 2 }` (already enabled) with the environment's directory, having written nothing to ours.
`NODE_DISABLE_COMPILE_CACHE` wins the same way and in the other direction: set anywhere above the
hook, there is no cache at all and the 35 ms comes back. Both are accepted rather than mitigated,
because an actor who can set either can also set `NODE_OPTIONS=--require ...` and run arbitrary code
in the hook: environment control is already total, and the compile cache adds nothing to it.

Measured on Node 22.16.0, `fault-grok`'s 21 hook invocations through `dist/oboete.mjs` -- the file
the installer writes -- with the two builds interleaved in the same session so machine load cancels:

| build | r1 | r2 | r3 | r4 | r5 | r6 | r7 |
|---|---|---|---|---|---|---|---|
| single file (`af871c9a`) | 218.9 | 217.1 | 214.8 | 220.5 | 216.7 | 211.3 | 212.4 |
| launcher + engine | 184.5 | 186.9 | 192.6 | 183.9 | 185.7 | 180.1 | 182.0 |

Medians in ms. Rounds 6 and 7 are the launcher as it ships; 4 and 5 the same with the earlier
ownership check, 1 to 3 with none. `oboete --help` alone goes 62 ms to 44 ms warm.

Moving the cache under the data directory re-opened the question those numbers answer, because the
fault harness gives every scenario a fresh home and the cache is now inside it: several of the 21
invocations pay a cold compile that the `~/.cache` build did not. Five more interleaved rounds, on
a busier machine (both arms sit about 18 ms higher than above, which is why they are interleaved):

| build | r1 | r2 | r3 | r4 | r5 |
|---|---|---|---|---|---|
| single file (`af871c9a`) | 236.9 | 234.4 | 234.3 | 238.6 | 237.3 |
| launcher + engine, cache under the home | 201.1 | 201.6 | 199.6 | 198.9 | 197.5 |

The gap is 35.8 ms at the median, against about 31 in the table above -- the launcher gains more on
the busier machine, not less. A cold first invocation per scenario is a cost the median absorbs.

The launcher build sits at the pre-US6 baseline, so compile cost accounted for the whole
regression. Note what that does *not* say: the 0008 schema still costs whatever it costs at
`openDatabase`, and the cache now compensates for it rather than removing it. The first hook after
an upgrade still finds an empty cache and pays the uncached ~222 ms once per version, inside the
bound.

Two accepted costs. The cache is unbounded -- Node keys entries on path and source hash and never
evicts, so a directory of a few megabytes per distinct build accumulates for developers who rebuild
often; it holds compiled code of this project and nothing from any fixture or capture, and it goes
away with the data directory, which is the one thing a user or CI image already knows how to clear.

And the suite got slower, until the harness caught up. `withTempHome` gives every test its own
`OBOETE_HOME`, so with the cache inside the home nearly every CLI-spawning test ran on a cold one
and paid the compile plus a cache write it never read back. Interleaved, three rounds each on the
`fault-*` suites: 42.4, 42.2, 42.4 s with the cache at `~/.cache`, 45.3, 44.7, 45.0 s with it under
the home -- about 6 %. CI turned the same cost into a failure. Over the 48 `took N ms` diagnostics
of a full `engine (24.x)` run the median went from 215.4 and 212.1 ms on the commit before the move
to 246.3 and 251.8 ms on the commit that made it -- +35 ms, which is the compile -- and on a runner
already sitting at ~215 ms against a 300 ms budget that was enough to fail `fault-storage`
`readonly` and `e2e-hook.test.ts:142` on both duplicate runs. What had been keeping them warm is
recorded a paragraph up -- the suite inherits `HOME` -- but the size of it was not: `test/helpers/fault.ts`
overrides `OBOETE_HOME` for each test and leaves `HOME` alone, so every spawn had been reading the
runner's own `~/.cache/oboete/compile`.

A cold cache per test is the harness's artefact and not the product's: a real installation's cache
outlives its invocations, so the way to measure the hook as it runs is to share one.
`package.json` loads `test/helpers/compile-cache.ts` into both `node --test` runs with `--import`,
and it sets `NODE_COMPILE_CACHE` to one `build/compile-cache` -- the variable this document records
the launcher cannot defend against, put to the use it is for -- and deletes
`NODE_DISABLE_COMPILE_CACHE`, which a developer's shell can carry and which would otherwise win
during child bootstrap and leave the timed suites measuring a hook nobody runs (with the flag set
and the deletion removed, every spawn is 217-229 ms against 189-193). Every test file and every CLI any of
them spawns inherits it, which is the point: the first attempt set it at three spawn sites and
missed the ad-hoc one in `test/fault-pi.test.ts` and the whole unit batch. Two rounds on the
`fault-*` suites again: 41.8, 41.6 s, under the `~/.cache` figure, so the one-directory rule costs
the suite nothing. `test/unit/launcher.test.ts` deletes the variable instead, because the directory
the launcher picks for itself is exactly what that suite is about.

`--import` covers one of the three places CI runs these tests: the `engine` job's `npm test`. The
`check` job runs the timed e2e bundle tests and the instrumented unit batch as steps of their own,
each with its own `node --test` command line and no `--import`, so what sets the variable there is
the module import itself -- `test/e2e-hook.test.ts`, `test/e2e-inject.test.ts` and
`test/helpers/fault.ts` all take `repositoryRoot` and `warmCompileCache` from that file. The unit batch imports none of it and runs cold there, which costs nothing: it
runs under `NODE_V8_COVERAGE`, where `WALL_CLOCK_IS_MEASURED` is false and no assertion reads a
time. What keeps the arrangement honest is `warmCompileCache`, which asserts the variable is the
directory the module chose: a suite that loses the import fails by name rather than by percentile,
which is the third time on this branch that an invisible cache state would otherwise have been paid
for in a red round.

Missing the unit batch is what the second CI round cost. It spawns the bundle a few dozen times and
had been leaving the runner's cache warm for the timed suites that run after it, so with only the
three sites wired the first spawn of the serial batch paid the whole compile: `e2e-hook.test.ts:110`
took 299.1 and 304.9 ms on the two duplicate runs and stored a partial row with a null `content`,
which is what a capture that runs out of its 300 ms does. The medians over the 48 invocations had
already come back to 219.4 and 223.3 ms from 246.3 and 251.8; the failure was the one spawn no
warm-up preceded. `.github/workflows/ci.yml` runs the e2e bundle tests as a step of their own --
alone and uninstrumented, which is what makes their numbers worth reading -- so there the first
spawn finds an empty cache whatever the runner did before it: 231.0 ms against 161.5 for the next
one, and 260.4 against 195.7 on the run after that -- both passed, both within 40 ms of the budget
for no reason a reader of the number would guess. `test/e2e-hook.test.ts`, `test/e2e-inject.test.ts` and `test/helpers/fault.ts`
therefore spend one throwaway run before their first timed one, which is where an installed oboete
pays it too -- once, at install. The fault suites warm through the helper rather than through
whichever file the runner happened to load first: run alone on an empty cache, `fault-storage`'s
first hook now takes 179.0 ms. Measured on the head that added it: the first timed spawn of that step took 204.6
and 210.2 ms on the two duplicate runs against 203.1 and 202.5 for the second spawn, where the head
before it had spent 260.4 against 195.7 and 211.6 against 162.5. The head after that reads 193.6
against 201.1 on one run and 214.9 against 161.9 on the other, whose whole series is noisier -- a
419.4 ms peak where every other run sits near 300 -- so what the warm-up removes is the systematic
cost, not the runner's variance.

Splitting one file into two put a new failure ahead of everything the engine does about its own:
the import. An engine that is missing or unreadable now throws in the launcher, above the handler
in `src/cli.ts` that gives `hook`, `capture` and `inject` their contracted exit 0. The launcher
therefore repeats that handler's shape for exactly those three commands -- exit 0, nothing on
stderr, one `logs/hook.log` line carrying the error's `code` and never its message -- and rethrows
for every other command, so a broken install stays loud for anyone checking by hand. Resolving this
file's own real path is inside the same `try`: a global upgrade unlinks and recreates the package
directory under a hook that is already running, and that line is the first to touch the disk
afterwards, so leaving it outside would have reopened the contract one line above the code that
closes it. Reading
`process.argv[2]` for the command name is the same thing `src/cli.ts` does in its own catch.

Pinned by `test/unit/launcher.test.ts`, which asserts the shape the speed-up depends on -- the entry
file is `src/launcher.mjs` verbatim, executable and small; the engine is its own file; the cache
follows `OBOETE_HOME` wherever it points -- unset, empty, blank, relative, dot-relative and with a
`..` in it, each against the home the real `resolveHome` returns -- and a relocated run leaves
nothing outside it; one run
leaves an owner-only cache, the next run adds nothing to it, and a run after that rewrites an entry
corrupted in between -- the last of those is what separates a cache being read back from one that
was never enabled, which is invisible to every other assertion; a world-writable cache directory is
refused, as is one merely traversable by others, one that is a symlink and one whose parent is a
symlink, while a data directory at 0755, 0775 and even 0777 is accepted and left at the mode it had,
as is a parent left loose at 0775; every directory the run creates, the data directory included, is
0700; the
one branch with no test is the parent's owner, because an unprivileged process cannot create a
directory that belongs to somebody else and a test that cannot fail is worse than none;
a home that cannot hold a cache still exits 0 with no stderr; an engine that cannot be imported
leaves each of `hook`, `capture` and `inject` at exit 0 with a hook-log line naming that command,
and `--version` loud and non-zero; and
`oboete setup` writes `dist/oboete.mjs` into the hook commands rather than the engine -- and not the
timing, which belongs to the machine.
