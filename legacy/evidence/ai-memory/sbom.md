# Phase 0A SBOM — ai-memory

## Snapshot and method

- Audited revision: `a9e9a24d50f59e970fc01ae48efe647abf20702e` (`v1.26.0`, detached HEAD).
- Dependency scope: every third-party normal, development, build, and target-specific direct dependency declared by a root-workspace member, plus the standalone companion importer in its own table. Internal path dependencies are described separately.
- Exact versions come from the checked-in `Cargo.lock` files. The direct sets were derived with `cargo metadata --no-deps` and deduplicated by package name: 50 root-workspace packages and 11 companion packages.
- License expressions are the package-version `licenseConcluded` values in GitHub's SPDX dependency-graph SBOM for `akitaonrails/ai-memory`, matched back to the exact locked name/version. Source: <https://api.github.com/repos/akitaonrails/ai-memory/dependency-graph/sbom> (retrieved 2026-08-12); the remote default-branch HEAD at retrieval was the same full commit audited here.
- This is a direct-dependency SBOM, not a legal opinion or a complete transitive license audit. `cargo deny`/`cargo audit` could not be installed or run because this environment could not resolve the Rust registries; the exact errors are in `upstream-test.log`.

## Repository license

- Root workspace: `MIT` (`Cargo.toml:19-24`; full text in `LICENSE:1-21`, copyright 2026 Fabio Akita).
- The ten product crates under `crates/` inherit `workspace.package.license = "MIT"` through `license.workspace = true`.
- `evals/` explicitly declares `MIT OR Apache-2.0` (`evals/Cargo.toml:1-7`). Only the root MIT license text is tracked; no Apache-2.0, NOTICE, or COPYING file was found.
- `companions/ai-memory-importer` is `publish = false` and has no `license` field (`companions/ai-memory-importer/Cargo.toml:1-7`). It resides under the repository's root MIT license, but its package manifest does not independently declare that license.

## Root workspace direct third-party dependencies (50)

Internal `ai-memory-*` path dependencies are omitted from this table; they are version `1.26.0` and inherit MIT as noted above.

| Package | Locked version | License expression |
|---|---:|---|
| anyhow | 1.0.103 | MIT OR Apache-2.0 |
| askama | 0.12.1 | MIT OR Apache-2.0 |
| async-trait | 0.1.89 | MIT OR Apache-2.0 |
| axum | 0.8.9 | MIT |
| base64 | 0.22.1 | MIT OR Apache-2.0 |
| clap | 4.6.1 | MIT OR Apache-2.0 |
| clap_complete | 4.6.7 | MIT OR Apache-2.0 |
| crossterm | 0.29.0 | MIT |
| dirs | 5.0.1 | MIT OR Apache-2.0 |
| figment | 0.10.19 | MIT OR Apache-2.0 |
| flate2 | 1.1.9 | MIT OR Apache-2.0 |
| fs2 | 0.4.3 | MIT OR Apache-2.0 |
| futures-util | 0.3.32 | MIT OR Apache-2.0 |
| getrandom | 0.3.4 | MIT OR Apache-2.0 |
| git2 | 0.21.0 | MIT OR Apache-2.0 |
| http | 1.4.0 | MIT OR Apache-2.0 |
| jiff | 0.2.24 | Unlicense OR MIT |
| jsonc-parser | 0.33.1 | MIT |
| notify | 8.2.0 | CC0-1.0 |
| notify-debouncer-full | 0.6.0 | MIT OR Apache-2.0 |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 |
| pulldown-cmark | 0.12.2 | MIT |
| refinery | 0.8.16 | MIT |
| regex | 1.12.3 | MIT OR Apache-2.0 |
| reqwest | 0.12.28 | MIT OR Apache-2.0 |
| rmcp | 1.7.0 | Apache-2.0 |
| rusqlite | 0.32.1 | MIT |
| schemars | 1.2.1 | MIT |
| secrecy | 0.10.3 | Apache-2.0 OR MIT |
| serde | 1.0.228 | MIT OR Apache-2.0 |
| serde_json | 1.0.150 | MIT OR Apache-2.0 |
| serde_urlencoded | 0.7.1 | MIT OR Apache-2.0 |
| serde_yaml | 0.9.34+deprecated | MIT OR Apache-2.0 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| subtle | 2.6.1 | BSD-3-Clause |
| sysinfo | 0.32.1 | MIT |
| tar | 0.4.46 | MIT OR Apache-2.0 |
| tempfile | 3.27.0 | MIT OR Apache-2.0 |
| thiserror | 2.0.18 | MIT OR Apache-2.0 |
| tokio | 1.52.3 | MIT |
| tokio-util | 0.7.18 | MIT |
| toml_edit | 0.22.27 | MIT OR Apache-2.0 |
| tower | 0.5.3 | MIT |
| tower-http | 0.6.11 | MIT |
| tracing | 0.1.44 | MIT |
| tracing-appender | 0.2.5 | MIT |
| tracing-subscriber | 0.3.23 | MIT |
| uuid | 1.23.1 | Apache-2.0 OR MIT |
| winapi-util | 0.1.11 | Unlicense OR MIT |
| wiremock | 0.6.5 | MIT OR Apache-2.0 |

## Standalone companion importer direct dependencies (11)

The importer is deliberately outside the root workspace and has a separate lockfile, so its versions are reported independently even where names overlap.

| Package | Locked version | License expression |
|---|---:|---|
| anyhow | 1.0.102 | MIT OR Apache-2.0 |
| clap | 4.6.1 | MIT OR Apache-2.0 |
| reqwest | 0.12.28 | MIT OR Apache-2.0 |
| serde | 1.0.228 | MIT OR Apache-2.0 |
| serde_json | 1.0.150 | MIT OR Apache-2.0 |
| serde_yaml | 0.9.34+deprecated | MIT OR Apache-2.0 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| tempfile | 3.27.0 | MIT OR Apache-2.0 |
| tokio | 1.52.3 | MIT |
| url | 2.5.8 | MIT OR Apache-2.0 |
| walkdir | 2.5.0 | Unlicense OR MIT |

## Native and binary assets

### Tracked artifacts

- 554 tracked files were inspected by name, Git mode, and `file` magic. No tracked ELF, Mach-O, PE, object file, shared library, WebAssembly module, Java archive, font, compressed archive, or prebuilt `ai-memory` executable was found.
- 86 files have Git executable mode; all are interpreted shell, PowerShell, or Python scripts rather than compiled binaries.
- Five tracked binary media assets are PNG documentation images:

| Asset | Size | Format/dimensions |
|---|---:|---|
| `docs/logo.png` | 125,960 B | PNG, 768×768, indexed color |
| `docs/logo-light.png` | 992,380 B | PNG, 1376×768, RGBA |
| `docs/logo-dark.png` | 1,590,707 B | PNG, 1376×768, RGBA |
| `docs/web-project-view.png` | 417,501 B | PNG, 2377×1413, RGB |
| `docs/web-projects-home.png` | 163,068 B | PNG, 2371×1086, RGB |

- Other generated/static assets are text: `docs/architecture-overview.svg` (10,058 B) and the checked-in `crates/ai-memory-web/static/tailwind.css` (19,923 B).

### Build-time native inputs and downloads

- SQLite is compiled from bundled source through direct dependency `rusqlite = 0.32.1` with `bundled, backup` features (`Cargo.toml:96-99`); the lockfile pins `libsqlite3-sys 0.30.1` (MIT).
- libgit2 is compiled from vendored source through direct dependency `git2 = 0.21.0` with `default-features = false, vendored-libgit2` (`Cargo.toml:129-130`); the lockfile pins `libgit2-sys 0.18.4+1.9.3` (MIT OR Apache-2.0).
- Other locked transitive native build inputs include `libz-sys 1.1.28` (MIT OR Apache-2.0) and `ring 0.17.14` (Apache-2.0 AND ISC; C/assembly). Platform `*-sys` crates provide OS API bindings; they are source dependencies, not tracked prebuilt blobs.
- `crates/ai-memory-web/build.rs:20-30` pins the external Tailwind CSS standalone CLI `3.4.17` (MIT; upstream license: <https://github.com/tailwindlabs/tailwindcss/blob/v3.4.17/LICENSE>). Downloads use `https://github.com/tailwindlabs/tailwindcss/releases/download/v3.4.17/<asset>` and the following source-pinned SHA-256 values (`build.rs:116-147`):

| Platform | Release asset | SHA-256 |
|---|---|---|
| Linux x86_64 | `tailwindcss-linux-x64` | `7d24f7fa191d2193b78cd5f5a42a6093e14409521908529f42d80b11fde1f1d4` |
| Linux aarch64 | `tailwindcss-linux-arm64` | `69b1378b8133192d7d2feb12a116fa12d035594f58db3eff215879e4ad8cf39b` |
| macOS x86_64 | `tailwindcss-macos-x64` | `6cbdad74be776c087ffa5e9a057512c54898f9fe8828d3362212dfe32fc933a3` |
| macOS aarch64 | `tailwindcss-macos-arm64` | `a1d0c7985759accca0bf12e51ac1dcbf0f6cf2fffb62e6e0f62d091c477a10a3` |
| Windows x86_64 | `tailwindcss-windows-x64.exe` | `67f1c5e3f5a03406a7bf5badf5ada09b79f3ae78ec43450c15f7e983068da346` |

  The build script downloads/executes the selected binary only when CSS regeneration is required (`build.rs:170-281`). No Tailwind executable is tracked in this revision; `TAILWIND_SKIP=1` uses the checked-in CSS (`build.rs:43-48`).
- Docker and native-packaging directories contain build/service recipes and scripts, not tracked image layers or package binaries.
