# Phase 0A SBOM evidence — remem

- Snapshot: `cde8bc05504c74794d044ef118f74d8f828adbf5`
- Package: `remem-ai` / `@remem-ai/remem` / Codex plugin `remem`, version `0.6.69`
- Generated: 2026-08-12 (Asia/Tokyo)
- Scope: repository license, direct dependencies, and native/binary assets. This is not a full transitive legal audit.

## Method and limitations

- Direct Rust dependencies come from `Cargo.toml:29-79`; exact selected versions come from the pinned `Cargo.lock`.
- The list includes normal dependencies, default-enabled optional dependencies, the Windows target dependency, and the dev dependency: 29 direct crates total.
- The isolated dependency installation could not reach `index.crates.io`; see `upstream-test.log`. License expressions therefore come from the exact-version crate registry manifests linked below, rather than locally downloaded crate sources.
- `npm/remem/package.json` has no `dependencies`, `optionalDependencies`, or `devDependencies`. The npm package and Codex plugin are distribution wrappers around the Rust binary and repository JavaScript.
- License strings are preserved as declared. In particular, legacy `MIT/Apache-2.0` declarations are not silently rewritten to an SPDX operator.

## Repository and package licenses

| Component | Version | License evidence |
|---|---:|---|
| Repository / Rust crate `remem-ai` | 0.6.69 | MIT — `LICENSE:1-20`, `Cargo.toml:1-7` |
| npm package `@remem-ai/remem` | 0.6.69 | MIT — `npm/remem/package.json:1-7` |
| Codex plugin `remem` | 0.6.69 | MIT — `plugins/remem/.codex-plugin/plugin.json:1-11` |

## Direct dependency licenses

| Direct dependency | Locked version | Scope | Declared license |
|---|---:|---|---|
| [anyhow](https://docs.rs/crate/anyhow/1.0.104) | 1.0.104 | runtime | MIT OR Apache-2.0 |
| [axum](https://docs.rs/crate/axum/0.8.9) | 0.8.9 | runtime | MIT |
| [base64](https://docs.rs/crate/base64/0.22.1) | 0.22.1 | runtime | MIT OR Apache-2.0 |
| [brush-parser](https://docs.rs/crate/brush-parser/0.4.0) | 0.4.0 | runtime | MIT |
| [chrono](https://docs.rs/crate/chrono/0.4.45) | 0.4.45 | runtime | MIT OR Apache-2.0 |
| [clap](https://docs.rs/crate/clap/4.6.4) | 4.6.4 | runtime | MIT OR Apache-2.0 |
| [dirs](https://docs.rs/crate/dirs/6.0.0) | 6.0.0 | runtime | MIT OR Apache-2.0 |
| [fastembed](https://docs.rs/crate/fastembed/5.17.3) | 5.17.3 | optional; default `local-onnx` | Apache-2.0 |
| [fs2](https://docs.rs/crate/fs2/0.4.3) | 0.4.3 | runtime | MIT/Apache-2.0 |
| [getrandom](https://docs.rs/crate/getrandom/0.3.4) | 0.3.4 | runtime | MIT OR Apache-2.0 |
| [hf-hub](https://docs.rs/crate/hf-hub/0.5.0) | 0.5.0 | optional; default `local-onnx` | Apache-2.0 |
| [libc](https://docs.rs/crate/libc/0.2.189) | 0.2.189 | runtime | MIT OR Apache-2.0 |
| [ndarray](https://docs.rs/crate/ndarray/0.17.2) | 0.17.2 | optional; default `local-onnx` | MIT OR Apache-2.0 |
| [ort](https://docs.rs/crate/ort/2.0.0-rc.12) | 2.0.0-rc.12 | optional; default `local-onnx` | MIT OR Apache-2.0 |
| [regex](https://docs.rs/crate/regex/1.13.1) | 1.13.1 | runtime | MIT OR Apache-2.0 |
| [regex-lite](https://docs.rs/crate/regex-lite/0.1.9) | 0.1.9 | runtime | MIT OR Apache-2.0 |
| [reqwest](https://docs.rs/crate/reqwest/0.12.28) | 0.12.28 | runtime | MIT OR Apache-2.0 |
| [ring](https://docs.rs/crate/ring/0.17.14) | 0.17.14 | runtime | Apache-2.0 AND ISC |
| [rmcp](https://docs.rs/crate/rmcp/0.15.0) | 0.15.0 | runtime | Apache-2.0 |
| [rusqlite](https://docs.rs/crate/rusqlite/0.32.1) | 0.32.1 | runtime | MIT |
| [serde](https://docs.rs/crate/serde/1.0.229) | 1.0.229 | runtime | MIT OR Apache-2.0 |
| [serde_json](https://docs.rs/crate/serde_json/1.0.151) | 1.0.151 | runtime | MIT OR Apache-2.0 |
| [sha2](https://docs.rs/crate/sha2/0.10.9) | 0.10.9 | runtime | MIT OR Apache-2.0 |
| [sqlite-vec](https://docs.rs/crate/sqlite-vec/0.1.9) | 0.1.9 | runtime | MIT/Apache-2.0 |
| [tokenizers](https://docs.rs/crate/tokenizers/0.22.2) | 0.22.2 | optional; default `local-onnx` | Apache-2.0 |
| [tokio](https://docs.rs/crate/tokio/1.53.1) | 1.53.1 | runtime | MIT |
| [toml_edit](https://docs.rs/crate/toml_edit/0.22.27) | 0.22.27 | runtime | MIT OR Apache-2.0 |
| [tower](https://docs.rs/crate/tower/0.5.3) | 0.5.3 | dev | MIT |
| [windows-sys](https://docs.rs/crate/windows-sys/0.61.2) | 0.61.2 | Windows target | MIT OR Apache-2.0 |

Direct-license count: 29 crates. npm direct-dependency count: 0.

## Tracked native and binary assets

The tracked-file scan found three binary media assets and no tracked executable, native library, WebAssembly module, or model-weight file (`*.so`, `*.dylib`, `*.dll`, `*.a`, `*.lib`, `*.exe`, `*.bin`, `*.wasm`, `*.onnx`, `*.gguf`). `npm/remem/bin/remem.js` is a text launcher, not a bundled native executable.

| Tracked asset | Type | Bytes | SHA-256 |
|---|---|---:|---|
| `assets/remem-demo.gif` | GIF89a, 1200×720 | 326,638 | `bbd096b17456a3b25515258fcdd22ac9fab6f27ea0a9f5a3fb22d12ddc44d698` |
| `assets/remem-recall-demo.gif` | GIF89a, 1200×760 | 330,326 | `acd93b839891f86f601b4437a1423cc7996cf7ef2ef349f51ee9499edeed3a5f` |
| `site/assets/remem-demo.gif` | GIF89a, 1200×720 | 521,563 | `046dab7f59b8d5e53107e8403d829e8e93c6e9e697486f19b82beaf6bdcee11e` |

`plugins/remem/runtimes/remem-releases.json:1-7` marks `0.6.69` as `unreleased` with an empty `assets` object, so the pinned plugin snapshot contains no checked release binary mapping.

## Native code and downloaded binary chain

These assets are not checked into this repository, but they can be compiled into or downloaded for a default build/runtime and therefore belong in the freeze inventory.

| Chain | Pinned evidence | Native/binary effect | Upstream license note |
|---|---|---|---|
| SQLCipher + vendored OpenSSL | `rusqlite 0.32.1` uses `bundled-sqlcipher-vendored-openssl` (`Cargo.toml:31`); lock contains `libsqlite3-sys 0.30.1`, `openssl-sys 0.9.117`, `openssl-src 300.6.1+3.6.3` | C SQLCipher/SQLite and OpenSSL are built and linked into the Rust binary | SQLCipher Community Edition uses a BSD 3-clause-style license; SQLite core is public domain; OpenSSL 3.6.3 is Apache-2.0. See [SQLCipher](https://github.com/sqlcipher/sqlcipher/blob/master/LICENSE.md), [SQLite notice](https://github.com/sqlcipher/sqlcipher/blob/master/SQLITE_LICENSE.md), [OpenSSL](https://github.com/openssl/openssl/blob/openssl-3.6.3/LICENSE.txt). |
| sqlite-vec | `sqlite-vec 0.1.9` depends on `cc 1.3.0` in `Cargo.lock` | Bundled C SQLite extension is compiled; direct crate license is in the table above | MIT/Apache-2.0 |
| ring | `ring 0.17.14` depends on `cc 1.3.0` in `Cargo.lock` | C/assembly implementation is compiled from crate sources | Apache-2.0 AND ISC |
| ONNX Runtime | Default feature enables `fastembed 5.17.3` and `ort 2.0.0-rc.12`; lock contains `ort-sys 2.0.0-rc.12`; fastembed enables `ort-download-binaries-rustls-tls` (`Cargo.toml:52`) | A platform ONNX Runtime binary is obtained during the default build and linked/packaged | ONNX Runtime is MIT; see [official license](https://github.com/microsoft/onnxruntime/blob/main/LICENSE). |
| Oniguruma | `tokenizers 0.22.2` enables `onig`; lock contains `onig 6.5.3` and `onig_sys 69.9.3`, which depends on `cc` | Oniguruma C code is compiled for tokenizer regex support | BSD 2-clause-style; see [Oniguruma COPYING](https://github.com/kkos/oniguruma/blob/v6.9.10/COPYING). |
| Local embedding model | `src/retrieval/embedding/local_semantic.rs:45-64,135-171` and `download.rs:9,66-94` | Explicit `remem embedding download` fetches `onnx/model.onnx` plus tokenizer/config files for `intfloat/multilingual-e5-small` at revision `614241f622f53c4eeff9890bdc4f31cfecc418b3`; no model is tracked in Git | The pinned model page declares MIT: [model revision](https://huggingface.co/intfloat/multilingual-e5-small/tree/614241f622f53c4eeff9890bdc4f31cfecc418b3). |

The release workflow builds four tarballs (`darwin-x64`, `darwin-arm64`, `linux-x64`, `linux-arm64`) at `.github/workflows/release.yml:38-88`. Intel macOS disables default `local-onnx`; the other three targets include it. npm and shell installers download a checksum-verified release tarball at install time (`npm/remem/scripts/install.js:14-17,140-172`; `install.sh:24-96`); no such tarball is present in this snapshot.
