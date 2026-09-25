//! Hook spike M14 (docs/spike/hook-m14.md): the cost of one hook write as a spawned process,
//! under synchronous=FULL (fullfsync on macOS), with the shipped redaction. Throwaway.

#[allow(dead_code)]
#[path = "../../../../src/redact.rs"]
mod redact;

#[allow(dead_code)]
mod hook {
    // redact.rs's `outbound` calls this; the spike times `redact` only.
    pub fn strip_blocks(text: &str, _typed: bool) -> String {
        text.to_string()
    }
}

use std::process::Command;
use std::time::Instant;

/// Tool-output-like text: paths, code, some Japanese, and every 20th line with words that wake
/// redaction rules (api, key, token, password) but no secret.
fn payload(size: usize) -> String {
    let mut s = String::with_capacity(size + 128);
    let mut i = 0u64;
    while s.len() < size {
        i += 1;
        if i % 20 == 0 {
            s.push_str(&format!("let api_key_path = config.token_file_{i}; // password is read elsewhere\n"));
        } else {
            s.push_str(&format!("src/module_{}.rs:{i}: let value_{i} = compute({}); // 計測用の行 {i}\n", i % 37, i * 7));
        }
    }
    let mut cut = size;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s
}

fn write_one(db: &str, size: usize, redact_on: bool, zstd_on: bool) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(2)).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;").unwrap();
    // HOOK_M14_NO_FULLFSYNC: the synchronous=FULL-only control on macOS (PR #68 review).
    if cfg!(target_os = "macos") && std::env::var_os("HOOK_M14_NO_FULLFSYNC").is_none() {
        conn.execute_batch("PRAGMA fullfsync=ON; PRAGMA checkpoint_fullfsync=ON;").unwrap();
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS raw(device TEXT NOT NULL, seq INTEGER NOT NULL, ts INTEGER NOT NULL,
         body BLOB NOT NULL, PRIMARY KEY(device, seq))",
    )
    .unwrap();
    let text = payload(size);
    let text = if redact_on { redact::redact(&text) } else { text };
    let body = if zstd_on { zstd::encode_all(text.as_bytes(), 3).unwrap() } else { text.into_bytes() };
    conn.execute(
        "INSERT INTO raw(device, seq, ts, body) VALUES('spike',
         (SELECT coalesce(max(seq), 0) + 1 FROM raw WHERE device = 'spike'), unixepoch(), ?1)",
        [body],
    )
    .unwrap();
}

fn spawn_once(exe: &std::path::Path, db: &str, size: usize, r: bool, z: bool) -> f64 {
    let flag = |b: bool| if b { "1" } else { "0" };
    let started = Instant::now();
    let status = Command::new(exe)
        .args(["one", db, &size.to_string(), flag(r), flag(z)])
        .status()
        .unwrap();
    assert!(status.success());
    started.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("one") => write_one(&args[2], args[3].parse().unwrap(), args[4] == "1", args[5] == "1"),
        Some("run") => {
            let (dir, n): (&str, usize) = (&args[2], args[3].parse().unwrap());
            std::fs::create_dir_all(dir).unwrap();
            let exe = std::env::current_exe().unwrap();
            println!("size_kb,redact,zstd,n,p50_ms,p95_ms,p99_ms,max_ms");
            for size in [1usize << 10, 64 << 10, 256 << 10] {
                for (r, z) in [(false, false), (true, false), (true, true)] {
                    let db = format!("{dir}/raw-{}-{}{}.db", size >> 10, r as u8, z as u8);
                    for suffix in ["", "-wal", "-shm"] {
                        let _ = std::fs::remove_file(format!("{db}{suffix}"));
                    }
                    spawn_once(&exe, &db, size, r, z); // warm the file cache and create the table
                    let mut ms: Vec<f64> = (0..n).map(|_| spawn_once(&exe, &db, size, r, z)).collect();
                    ms.sort_by(f64::total_cmp);
                    let p = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
                    println!("{},{r},{z},{n},{:.1},{:.1},{:.1},{:.1}", size >> 10, p(0.5), p(0.95), p(0.99), ms[n - 1]);
                }
            }
        }
        _ => eprintln!("usage: hook-m14 run <dir> <n> | one <db> <bytes> <redact 0|1> <zstd 0|1>"),
    }
}
