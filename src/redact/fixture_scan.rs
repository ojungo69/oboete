use super::*;

/// Reports what the rules mask in the fixture of record; skipped when it is not checked out.
#[test]
fn fixture_masks_are_plausible() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../free-mem/test/fixtures/events-1000.jsonl"
    );
    let Ok(text) = std::fs::read_to_string(path) else {
        eprintln!("fixture missing, skipped");
        return;
    };
    let started = std::time::Instant::now();
    let mut masked = 0;
    let mut samples = Vec::new();
    for line in text.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let payload = v["payload"].to_string();
        let r = redact(&payload);
        if r != payload {
            masked += 1;
            if samples.len() < 8 {
                let at = r.find(MASK).unwrap();
                let lo = r[..at].char_indices().rev().nth(60).map_or(0, |(i, _)| i);
                let hi = r[at..]
                    .char_indices()
                    .nth(40)
                    .map_or(r.len(), |(i, _)| at + i);
                samples.push(r[lo..hi].to_string());
            }
        }
    }
    eprintln!(
        "fixture: {} lines, {masked} masked, {} ms\n{}",
        text.lines().count(),
        started.elapsed().as_millis(),
        samples.join("\n---\n")
    );
}
