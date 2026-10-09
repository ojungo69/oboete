//! Milestone 4, Task 10's spike (docs/spike/local-embeddings.md): bge-m3 run in this process with
//! fastembed, from the pinned files in one directory, never fetched by fastembed itself. Built
//! only with the `local-embed` feature.
//!
//! The graph is not BAAI's: `assets/bge-m3-mha.onnx` is BAAI's with its attention fused into
//! ONNX Runtime's `MultiHeadAttention` (docs/spike/local-embeddings/fuse.py), which runs as
//! FlashAttention and keeps memory linear in the text's length. BAAI's graph needed 7.9 GB for
//! one text of 6,706 tokens; this one 2.4 GB at 8,192, with the same vectors. Its weights are
//! BAAI's file, unchanged.
//!
//! ONNX Runtime is not linked into the binary (fastembed's `ort-load-dynamic`): a process that
//! embeds loads Microsoft's released library from the model's `onnxruntime/` folder, checked
//! against its pin first, so a process that does not embed (a hook) pays nothing for it.

use std::io::{BufRead, Read};
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use fastembed::{
    InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// huggingface.co/BAAI/bge-m3 at this commit (MIT): the tokenizer's files and the weights.
pub const COMMIT: &str = "5617a9f61b028005a4858fdac845db406aefb181";

/// The fused graph, and its SHA-256 as `fuse.py` wrote it.
const GRAPH: &[u8] = include_bytes!("../assets/bge-m3-mha.onnx");
const GRAPH_SHA256: &str = "395d177d56b5eb75c0cdd0b87bf9b939652e0a424b3257ba31c8b7fe848e1654";

/// Each file's path in the model's directory, size and SHA-256.
pub const FILES: &[(&str, u64, &str)] = &[
    (
        "config.json",
        687,
        "26159e7ad065073448460117eb24b7a4572f6f4e78eadff65dc0a11c052449fa",
    ),
    (
        "special_tokens_map.json",
        964,
        "8c785abebea9ae3257b61681b4e6fd8365ceafde980c21970d001e834cf10835",
    ),
    (
        "tokenizer_config.json",
        444,
        "a62b2b6784f990259fddef5f16388693a8043be4f69179e6a5257eeb3f9abac4",
    ),
    (
        "tokenizer.json",
        17_098_108,
        "21106b6d7dab2952c1d496fb21d5dc9db75c28ed361a05f5020bbba27810dd08",
    ),
    (
        "onnx/model.onnx_data",
        2_266_820_608,
        "1eebfb28493f67bba03ce0ef64bfdc7fc5a3bd9d7493f818bb1d78cd798416b4",
    ),
];

/// The tokens a text is cut to, Workers AI's limit too.
pub const MAX_LENGTH: usize = 8192;

/// Microsoft's ONNX Runtime 1.28.0 library for this target, in the model's `onnxruntime/` folder:
/// its name, size and SHA-256, taken from the release archive whose digest GitHub publishes.
/// `None` where Microsoft releases none (macOS x64): that target gets no `local`.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const RUNTIME: Option<(&str, u64, &str)> = Some((
    "libonnxruntime.so.1.28.0",
    24_268_848,
    "1461ef7cc3d9e49982591721683cc3e3a55580aeca9a5254e7aac47b75ee4bab",
));
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub const RUNTIME: Option<(&str, u64, &str)> = Some((
    "libonnxruntime.so.1.28.0",
    20_591_712,
    "f1ec1a08eb99bd6e5401340f0a2b101381bf4694415480291dc13bcaa30f9ec7",
));
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const RUNTIME: Option<(&str, u64, &str)> = Some((
    "libonnxruntime.1.28.0.dylib",
    39_312_136,
    "dc19bbcb2f5c9fb3c68b4f9248aa0a35065ff702c5dbeae75eac54a74da97b6d",
));
#[cfg(all(windows, target_arch = "x86_64"))]
pub const RUNTIME: Option<(&str, u64, &str)> = Some((
    "onnxruntime.dll",
    15_809_848,
    "18370c375f07357fa5874344a9d9ac17e6b6fe1eb18b1dd209d79483b4470257",
));
#[cfg(not(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(target_os = "macos", target_arch = "aarch64"),
    all(windows, target_arch = "x86_64")
)))]
pub const RUNTIME: Option<(&str, u64, &str)> = None;

/// A file at its pinned size and SHA-256.
fn check(path: &Path, size: u64, sha: &str) -> Result<()> {
    let name = path.display();
    let mut f = std::fs::File::open(path).with_context(|| format!("opening {name}"))?;
    let len = f.metadata()?.len();
    if len != size {
        bail!("{name}: {len} bytes, not {size}");
    }
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    let got = format!("{:x}", hash.finalize());
    if got != sha {
        bail!("{name}: SHA-256 {got}, not the pinned {sha}");
    }
    Ok(())
}

/// The runtime's library in the model's directory, and its pin.
fn runtime_file(dir: &Path) -> Result<(std::path::PathBuf, u64, &'static str)> {
    let (name, size, sha) =
        RUNTIME.context("Microsoft releases no ONNX Runtime for this target")?;
    Ok((dir.join("onnxruntime").join(name), size, sha))
}

/// The graph, the runtime's library and every file at their pinned size and hash.
pub fn verify(dir: &Path) -> Result<()> {
    let got = format!("{:x}", Sha256::digest(GRAPH));
    if got != GRAPH_SHA256 {
        bail!("the fused graph: SHA-256 {got}, not the pinned {GRAPH_SHA256}");
    }
    let (path, size, sha) = runtime_file(dir)?;
    check(&path, size, sha)?;
    for (name, size, sha) in FILES {
        check(&dir.join(name), *size, sha)?;
    }
    Ok(())
}

/// ONNX Runtime's library, loaded once per process, checked against its pin just before.
fn runtime(dir: &Path) -> Result<()> {
    static LOADED: std::sync::OnceLock<std::result::Result<(), String>> =
        std::sync::OnceLock::new();
    LOADED
        .get_or_init(|| {
            let (path, size, sha) = runtime_file(dir).map_err(|e| format!("{e:#}"))?;
            check(&path, size, sha).map_err(|e| format!("{e:#}"))?;
            let path = std::path::absolute(&path).map_err(|e| e.to_string())?;
            ort::init_from(path).map_err(|e| e.to_string())?.commit();
            Ok(())
        })
        .clone()
        .map_err(anyhow::Error::msg)
}

/// The model from `dir`'s files, with `threads` for ONNX Runtime (all the machine's when `None`).
pub fn load(dir: &Path, threads: Option<usize>) -> Result<TextEmbedding> {
    runtime(dir)?;
    let read =
        |name: &str| std::fs::read(dir.join(name)).with_context(|| format!("reading {name}"));
    let tokenizer = TokenizerFiles {
        tokenizer_file: read("tokenizer.json")?,
        config_file: read("config.json")?,
        special_tokens_map_file: read("special_tokens_map.json")?,
        tokenizer_config_file: read("tokenizer_config.json")?,
    };
    let model =
        UserDefinedEmbeddingModel::new(GRAPH.to_vec(), tokenizer).with_pooling(Pooling::Cls);
    // The weights stay in their file: ONNX Runtime reads the external data from the model's
    // folder instead of a copy in memory, which doubled the peak (5.6 GB for one query).
    let folder = dir.join("onnx");
    let mut options = InitOptionsUserDefined::new()
        .with_max_length(MAX_LENGTH)
        .with_session_config(
            "session.model_external_initializers_file_folder_path",
            folder.to_str().context("the model's folder is not UTF-8")?,
        );
    if let Some(n) = threads {
        options = options.with_intra_threads(n);
    }
    Ok(TextEmbedding::try_new_from_user_defined(model, options)?)
}

/// One text at a time: the fused graph has no attention mask, so the padding a batch adds to its
/// shorter texts would change their vectors.
pub fn embed(model: &mut TextEmbedding, text: &str) -> Result<Vec<f32>> {
    let mut out = model.embed([text], Some(1))?;
    out.pop().context("no vector")
}

/// `oboete embed-spike`: `{"id","text"}` lines on stdin, each text through the outbound gate,
/// `{"id","vec"}` lines out; load time, each text's time and the peak resident set on stderr.
pub fn spike(dir: &Path, threads: Option<usize>, check: bool) -> Result<()> {
    if check {
        let t = Instant::now();
        verify(dir)?;
        eprintln!("verify_ms {}", t.elapsed().as_millis());
    }
    let mut rows = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let v: Value = serde_json::from_str(&line?)?;
        let id = v["id"].as_str().context("id")?.to_owned();
        let text = crate::redact::outbound(v["text"].as_str().context("text")?);
        rows.push((id, text));
    }
    let t = Instant::now();
    let mut model = load(dir, threads)?;
    eprintln!("bge-m3 {COMMIT} load_ms {}", t.elapsed().as_millis());
    let mut ms = Vec::new();
    for (id, text) in &rows {
        let t = Instant::now();
        let vec = embed(&mut model, text)?;
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
        println!("{}", json!({"id": id, "vec": vec}));
    }
    if !ms.is_empty() {
        let mut sorted = ms.clone();
        sorted.sort_by(f64::total_cmp);
        let at =
            |q: f64| sorted[((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1];
        eprintln!(
            "embed_ms n {} p50 {:.1} p95 {:.1} max {:.1}",
            sorted.len(),
            at(0.50),
            at(0.95),
            at(1.0)
        );
    }
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    if let Some(l) = status.lines().find(|l| l.starts_with("VmHWM")) {
        eprintln!("{l}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cos(a: &[f32], b: &[f32]) -> f64 {
        let dot: f64 = a
            .iter()
            .zip(b)
            .map(|(x, y)| f64::from(*x) * f64::from(*y))
            .sum();
        let norm = |v: &[f32]| v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
        dot / (norm(a) * norm(b))
    }

    fn top10(q: &[f32], docs: &[Vec<f32>]) -> Vec<usize> {
        let mut scored: Vec<(f64, usize)> = docs
            .iter()
            .enumerate()
            .map(|(i, d)| (cos(q, d), i))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().take(10).map(|(_, i)| i).collect()
    }

    /// Line A's fixture (docs/spike/local-embeddings.md): the public texts and queries of
    /// `src/testdata/embed-agreement/` against their stored Workers AI vectors: each cos at least
    /// 0.99, and each query's top 10 over the texts overlaps in 9. The model's directory comes from
    /// `OBOETE_BGE_M3`.
    #[test]
    #[ignore]
    fn local_vectors_agree_on_the_fixture() {
        let dir = std::env::var_os("OBOETE_BGE_M3").expect("OBOETE_BGE_M3 names the model's files");
        let dir = Path::new(&dir);
        verify(dir).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/embed-agreement");
        let rows: Vec<Value> = std::fs::read_to_string(fixture.join("items.jsonl"))
            .unwrap()
            .split('\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let raw = std::fs::read(fixture.join("workers-ai.f32")).unwrap();
        assert_eq!(raw.len(), rows.len() * 1024 * 4);
        let stored: Vec<Vec<f32>> = raw
            .as_chunks::<{ 1024 * 4 }>()
            .0
            .iter()
            .map(|c| {
                c.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect()
            })
            .collect();
        let mut model = load(dir, None).unwrap();
        let local: Vec<Vec<f32>> = rows
            .iter()
            .map(|r| embed(&mut model, r["text"].as_str().unwrap()).unwrap())
            .collect();
        let mut worst = (f64::MAX, String::new());
        for ((r, l), s) in rows.iter().zip(&local).zip(&stored) {
            let c = cos(l, s);
            if c < worst.0 {
                worst = (c, r["id"].as_str().unwrap().to_owned());
            }
        }
        let is_query = |r: &Value| r["kind"] == "query";
        let docs: Vec<usize> = (0..rows.len()).filter(|&i| !is_query(&rows[i])).collect();
        let pick = |v: &Vec<Vec<f32>>| docs.iter().map(|&i| v[i].clone()).collect::<Vec<_>>();
        let (local_docs, stored_docs) = (pick(&local), pick(&stored));
        let mut overlaps = Vec::new();
        for (i, r) in rows.iter().enumerate().filter(|(_, r)| is_query(r)) {
            let want = top10(&stored[i], &stored_docs);
            for got in [
                top10(&local[i], &stored_docs),
                top10(&local[i], &local_docs),
            ] {
                overlaps.push((
                    want.iter().filter(|d| got.contains(d)).count(),
                    r["id"].clone(),
                ));
            }
        }
        let fewest = overlaps.iter().min_by_key(|o| o.0).unwrap();
        eprintln!(
            "items {}, queries {}, cos min {:.5} ({}), top-10 overlap min {} ({})",
            rows.len(),
            rows.len() - docs.len(),
            worst.0,
            worst.1,
            fewest.0,
            fewest.1
        );
        assert!(worst.0 >= 0.99, "cos {:.5} at {}", worst.0, worst.1);
        assert!(fewest.0 >= 9, "top-10 overlap {} at {}", fewest.0, fewest.1);
    }
}
