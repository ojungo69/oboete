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

use std::io::BufRead;
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

/// The tokens a text is cut to, Workers AI's limit too.
pub const MAX_LENGTH: usize = 8192;

/// This target's runtime library and its pin (`model_fetch::RUNTIME`).
fn library() -> Result<&'static crate::model_fetch::Artifact> {
    crate::model_fetch::RUNTIME
        .as_ref()
        .map(|runtime| &runtime.library)
        .context("Microsoft releases no ONNX Runtime 1.28.0 for this target")
}

/// The graph, the runtime's library and every model file at their pinned size and hash.
pub fn verify(dir: &Path) -> Result<()> {
    let got = format!("{:x}", Sha256::digest(GRAPH));
    if got != GRAPH_SHA256 {
        bail!("the fused graph: SHA-256 {got}, not the pinned {GRAPH_SHA256}");
    }
    for artifact in crate::model_fetch::BGE_M3.iter().chain([library()?]) {
        crate::model_fetch::check(&dir.join(artifact.name), artifact)?;
    }
    Ok(())
}

/// ONNX Runtime's library, loaded once per process, checked against its pin just before. Only a
/// success is kept: a resident worker loads again once setup has put the library right.
fn runtime(dir: &Path) -> Result<()> {
    static LOADED: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
    let mut loaded = LOADED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !*loaded {
        let library = library()?;
        let path = dir.join(library.name);
        crate::model_fetch::check(&path, library)?;
        ort::init_from(std::path::absolute(&path)?)?.commit();
        *loaded = true;
    }
    Ok(())
}

/// The model in `dir` as a `Resident` loads it, on at most 8 of the machine's threads (the
/// spike's cap: its memory and agreement were measured there).
pub fn loader(dir: std::path::PathBuf) -> crate::resident::Load {
    Box::new(move || {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
        let mut model = load(&dir, Some(threads))?;
        Ok(Box::new(move |text: &str| embed(&mut model, text)) as crate::resident::Model)
    })
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
mod tests;
