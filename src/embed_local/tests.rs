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
