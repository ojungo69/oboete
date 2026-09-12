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

The cache directory is `$XDG_CACHE_HOME/oboete/compile`, or `~/.cache/oboete/compile` when that
variable is unset or relative, created with mode 0700. It is deliberately neither of the two obvious
alternatives. It is not under `OBOETE_HOME`, because the cache belongs to
the build rather than to a data directory and the fault harness gives every scenario a fresh home --
a cache under the home would be cold on each scenario and the fix would show up only in `--help`.
It is not Node's own default either: with no argument `enableCompileCache()` uses
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
follows a link, so a link planted at the `oboete` directory would have the launcher create `compile`
inside somebody else's tree and write half a megabyte of bytecode there while every check on
`compile` itself passed -- it would be ours, 0700, a real directory. The parent is therefore checked
*before* `compile` is created, so a refusal leaves nothing behind at all. No mode is required of it:
`compile`'s own traverse bits already decide who can reach the entries.

That last sentence is a test, not a preference. Twice while writing this fix a mode requirement went
onto the `oboete` directory, and both times every other pin stayed green while the cache went off on
every machine that already had one: the launcher creates that directory itself at 0700, which passes
any mode check at any umask, so only a directory left behind by an earlier release -- made by a
recursive `mkdir` that carried no mode, hence 0777 minus the umask -- ever fails one. The pin makes
that directory at 0775 and asserts the run still fills a cache.

Nothing above that is checked. Two rounds of this review tried requiring the rest of the ancestors
to be unwritable by others and both times it refused real machines -- a `~/.cache` is 0755 nearly
everywhere and 0775 wherever a 002 umask created it -- while buying very little: a symlink at
`~/.cache` is the user's own arrangement, and replacing one takes write access to `$HOME`, which is
total compromise on its own. What write access above buys is the race between the check and V8's
read, and that race is accepted anyway; `homedir()` is not checked either, so demanding unwritable
ancestors would have narrowed it rather than closed it.

`~/.cache` is created at whatever the umask gives it, while `oboete` and `compile` below it are
created at 0700: on a fresh account or a container image the capture hook may be the first thing on
the machine to create `~/.cache`, and that directory belongs to every tool, not to this one. An
existing `~/.cache` is left exactly as found, whatever its mode.

The one refusal that stays silent is a `compile` of this user's own left at a loose mode -- a
restored backup, an `rsync` without `-p`, an NFS home. The hook returns to its uncached time with
nothing saying so; `scripts/measure-cold-start.mjs` prints the directory and whether it was non-empty
-- which is a fact about the directory, not about whether the launcher accepted it -- and
`oboete doctor` does not yet have an item for it. Refusing is deliberate: correcting
the mode would chmod the target of whatever symlink was planted there. What remains is the ordinary
race between the check and V8's read, which needs write access to the cache directory itself -- or
to any directory above it, `$HOME` included, none of which is checked. A writable `$HOME` is already
total compromise, so this is the accepted residue rather than a hole the ancestor rule would have
closed. A home that cannot hold the directory at all costs the cache and never the command.

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
`%USERPROFILE%\.cache\oboete\compile`, or an `XDG_CACHE_HOME` override, is trusted on the strength
of its path alone.

One thing the launcher cannot defend: `NODE_COMPILE_CACHE` in the environment wins. Node enables the
cache at bootstrap from that variable, and a later `enableCompileCache(dir)` returns
`{ status: 2 }` (already enabled) with the environment's directory, having written nothing to ours.
That is accepted rather than mitigated, because an actor who can set that variable can also set
`NODE_OPTIONS=--require ...` and run arbitrary code in the hook: environment control is already
total, and the compile cache adds nothing to it.

Measured on Node 22.16.0, `fault-grok`'s 21 hook invocations through `dist/oboete.mjs` -- the file
the installer writes -- with the two builds interleaved in the same session so machine load cancels:

| build | r1 | r2 | r3 | r4 | r5 | r6 | r7 |
|---|---|---|---|---|---|---|---|
| single file (`af871c9a`) | 218.9 | 217.1 | 214.8 | 220.5 | 216.7 | 211.3 | 212.4 |
| launcher + engine | 184.5 | 186.9 | 192.6 | 183.9 | 185.7 | 180.1 | 182.0 |

Medians in ms. Rounds 6 and 7 are the launcher as it ships; 4 and 5 the same with the earlier
ownership check, 1 to 3 with none. `oboete --help` alone goes 62 ms to 44 ms warm.

The launcher build sits at the pre-US6 baseline, so compile cost accounted for the whole
regression. Note what that does *not* say: the 0008 schema still costs whatever it costs at
`openDatabase`, and the cache now compensates for it rather than removing it. The first hook after
an upgrade still finds an empty cache and pays the uncached ~222 ms once per version, inside the
bound.

Two accepted costs. The cache is unbounded -- Node keys entries on path and source hash and never
evicts, so a directory of a few megabytes per distinct build accumulates for developers who rebuild
often; it holds compiled code of this project and nothing from any fixture or capture, and
`~/.cache` is where a user or CI image already expects to clear such a thing. And the test suite warms the
developer's real `~/.cache`, because most CLI-spawning tests inherit `HOME`: giving each scenario its
own would make the cache cold in exactly the suite whose timing this section is about, so those
suites deliberately measure the hook the way it actually runs. Two suites that spawn the CLI do
override `HOME` (`cli`, `fault-pi`) and consequently run uncached, paying the compile and a
discarded cache write on every spawn; `launcher` overrides it deliberately, since a cache directory
is what it is testing.

Pinned by `test/unit/launcher.test.ts`, which asserts the shape the speed-up depends on -- the entry
file is `src/launcher.mjs` verbatim, executable and small; the engine is its own file; one run
leaves an owner-only cache, the next run adds nothing to it, and a run after that rewrites an entry
corrupted in between -- the last of those is what separates a cache being read back from one that
was never enabled, which is invisible to every other assertion; a world-writable cache directory is
refused, as is one merely traversable by others, one that is a symlink and one whose parent is a
symlink, while ancestors at 0755, 0775 and even 0777 are accepted and left at the mode they had; the
one branch with no test is the parent's owner, because an unprivileged process cannot create a
directory that belongs to somebody else and a test that cannot fail is worse than none;
a home that cannot hold a cache still exits 0 with no stderr; and
`oboete setup` writes `dist/oboete.mjs` into the hook commands rather than the engine -- and not the
timing, which belongs to the machine.
