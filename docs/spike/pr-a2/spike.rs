//! PR-A2 measurement spike, built only with `--features spike` and never shipped. Reads
//! `{"id","text"}` lines on stdin, passes each text through the outbound gate, embeds it with
//! fastembed's bge-m3 and prints `{"id","text","vec"}` lines. Load time, embedding time and
//! peak memory go to stderr.

use std::io::BufRead;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use serde_json::{Value, json};

pub fn embed(cache: PathBuf, max_length: usize) -> Result<()> {
    let mut rows = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let v: Value = serde_json::from_str(&line?)?;
        let id = v["id"].as_str().context("id")?.to_string();
        let text = crate::redact::outbound(v["text"].as_str().context("text")?);
        rows.push((id, text));
    }
    let t = Instant::now();
    let mut model = TextEmbedding::try_new(
        TextInitOptions::new(EmbeddingModel::BGEM3)
            .with_cache_dir(cache)
            .with_max_length(max_length)
            .with_show_download_progress(true),
    )?;
    eprintln!("load_ms {}", t.elapsed().as_millis());
    let t = Instant::now();
    let texts: Vec<&str> = rows.iter().map(|(_, t)| t.as_str()).collect();
    let vecs = model.embed(&texts, Some(8))?;
    eprintln!("embed_ms {} docs {}", t.elapsed().as_millis(), rows.len());
    for ((id, text), vec) in rows.iter().zip(vecs) {
        println!("{}", json!({"id": id, "text": text, "vec": vec}));
    }
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    if let Some(l) = status.lines().find(|l| l.starts_with("VmHWM")) {
        eprintln!("{l}");
    }
    Ok(())
}
