//! Mask well-known secret shapes before text is stored or sent anywhere.
//! M0 carries a handful of patterns; M1 vendors the gitleaks rule set.
//! ponytail: hand-written matchers, swap for the gitleaks TOML + regex crate in M1.

const MASK: &str = "[REDACTED]";

/// Prefix-based token shapes: (prefix, minimum total length).
const PREFIXES: &[(&str, usize)] = &[
    ("sk-", 20),    // OpenAI / Anthropic-style keys
    ("gsk_", 20),   // Groq
    ("nvapi-", 20), // NVIDIA NIM
    ("ghp_", 30),   // GitHub PAT
    ("github_pat_", 30),
    ("xoxb-", 20), // Slack
    ("xoxp-", 20),
    ("AKIA", 20),   // AWS access key id
    ("AIza", 35),   // Google API key
    ("glpat-", 20), // GitLab
];

pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some((start, end)) = next_secret(rest) {
            out.push_str(&rest[..start]);
            out.push_str(MASK);
            rest = &rest[end..];
        } else {
            out.push_str(rest);
            break;
        }
    }
    out
}

/// Byte range of the earliest secret in `s`, if any.
fn next_secret(s: &str) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    let mut consider = |range: Option<(usize, usize)>| {
        if let Some(r) = range
            && best.is_none_or(|b| r.0 < b.0)
        {
            best = Some(r);
        }
    };
    consider(private_key_block(s));
    consider(bearer_token(s));
    for (prefix, min_len) in PREFIXES {
        consider(prefixed_token(s, prefix, *min_len));
    }
    best
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '+' | '=')
}

fn token_end(s: &str, start: usize) -> usize {
    s[start..]
        .char_indices()
        .find(|(_, c)| !is_token_char(*c))
        .map(|(i, _)| start + i)
        .unwrap_or(s.len())
}

fn prefixed_token(s: &str, prefix: &str, min_len: usize) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(pos) = s[from..].find(prefix) {
        let start = from + pos;
        let boundary_ok = start == 0 || !s[..start].ends_with(is_token_char);
        let end = token_end(s, start);
        if boundary_ok && end - start >= min_len {
            return Some((start, end));
        }
        from = start + prefix.len();
    }
    None
}

fn bearer_token(s: &str) -> Option<(usize, usize)> {
    let pos = s.find("Bearer ")?;
    let start = pos + "Bearer ".len();
    let end = token_end(s, start);
    (end - start >= 16).then_some((start, end))
}

fn private_key_block(s: &str) -> Option<(usize, usize)> {
    const TAIL: &str = "PRIVATE KEY-----";
    let start = s.find("-----BEGIN ")?;
    let head_end = start + s[start..].find(TAIL)? + TAIL.len();
    let end = match s[head_end..].find("-----END ") {
        Some(e) => {
            let e = head_end + e;
            s[e..]
                .find(TAIL)
                .map(|k| e + k + TAIL.len())
                .unwrap_or(s.len())
        }
        None => s.len(),
    };
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_known_shapes_and_leaves_prose() {
        let s = "key gsk_abcdefghijklmnopqrstuvwxyz0123 and Bearer abcdefghijklmnopqrstu done";
        let r = redact(s);
        assert_eq!(r, "key [REDACTED] and Bearer [REDACTED] done");
        assert_eq!(
            redact("no secrets here, task-1 ok"),
            "no secrets here, task-1 ok"
        );
        let pem = "x -----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY----- y";
        assert_eq!(redact(pem), "x [REDACTED] y");
        assert_eq!(redact("AKIAIOSFODNN7EXAMPLE1234"), "[REDACTED]");
        assert_eq!(
            redact("risky-AKIAIOSFODNN7EXAMPLE1234"),
            "risky-AKIAIOSFODNN7EXAMPLE1234"
        );
    }
}
