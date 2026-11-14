use super::*;

#[test]
fn batches_stay_under_the_request_limits() {
    let doc = |n: usize| (String::new(), "x".repeat(n));
    let mut todo: Vec<_> = (0..250).map(|_| doc(10)).collect();
    todo.extend((0..30).map(|_| doc(2_000)));
    todo.push(doc(60_000));
    let got = batches(&todo);
    for b in &got {
        let longest = b.iter().map(|(_, t)| t.len()).max().unwrap();
        assert!(b.len() <= BATCH);
        assert!(
            b.len() == 1 || b.len() * longest <= BATCH_CHARS,
            "{}",
            b.len()
        );
    }
    assert_eq!(got.iter().map(|b| b.len()).sum::<usize>(), todo.len());
    assert_eq!(got.last().unwrap().len(), 1);
}

/// Task 10: doctor names `local`'s files from their sizes and the marker, hashing nothing:
/// not downloaded, incomplete (which), changed since verified, ready; and a build that cannot
/// run them.
#[test]
fn doctor_names_the_local_model_state() {
    use crate::model_fetch::Artifact;
    let pin = |b: &[u8]| -> &'static str {
        Box::leak(format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(b)).into_boxed_str())
    };
    let files = [
        Artifact {
            name: "a.json",
            url: "",
            size: 1,
            sha256: pin(b"a"),
        },
        Artifact {
            name: "onnx/b",
            url: "",
            size: 2,
            sha256: pin(b"bb"),
        },
    ];
    let home = tempfile::tempdir().unwrap();
    let dir = local_dir(home.path());
    let state = || files_state(&dir, &files);
    assert!(
        !state().0 && state().1.contains("not downloaded"),
        "{:?}",
        state()
    );
    std::fs::create_dir_all(dir.join("onnx")).unwrap();
    std::fs::write(dir.join("a.json"), "a").unwrap();
    assert!(state().1.contains("incomplete") && state().1.contains("onnx/b missing"));
    std::fs::write(dir.join("onnx/b"), "bb").unwrap();
    assert!(
        !state().0 && state().1.contains("not verified"),
        "{:?}",
        state()
    );
    crate::model_fetch::verify(&dir, &files).unwrap();
    assert!(state().0 && state().1.contains("ready"), "{:?}", state());
    // Written again, same size: the marker no longer matches, and nothing was hashed to say so.
    let file = dir.join("a.json");
    let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
    std::fs::remove_file(&file).unwrap();
    std::fs::write(&file, "x").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(modified + Duration::from_secs(10))
        .unwrap();
    assert!(
        !state().0 && state().1.contains("not verified"),
        "{:?}",
        state()
    );
    let (ready, line) = local_state(home.path());
    if let Some(why) = local_unavailable() {
        assert_eq!((ready, line), (false, format!("local model: {why}")));
    } else {
        assert!(!ready && line.contains("not downloaded"), "{line}");
    }
}

/// Task 10: both runners' vectors pass `unit` (Workers AI's answers through `vectors`, the
/// local model's on its `Resident`): 1,024 finite numbers, not all zero, scaled to length 1.
#[test]
fn a_bad_vector_is_refused_by_either_runner() {
    let v = unit(vec![3.0; DIM]).unwrap();
    assert!((v.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-5);
    assert!(unit(vec![1.0; DIM - 1]).is_err());
    assert!(unit(vec![0.0; DIM]).is_err());
    for bad in [f32::NAN, f32::INFINITY] {
        let mut v = vec![1.0; DIM];
        v[7] = bad;
        assert!(unit(v).is_err());
    }
}

#[test]
fn answers_with_bad_coordinates_are_refused() {
    let row = |x: Value| {
        let mut r: Vec<Value> = vec![json!(0.5); DIM];
        r[3] = x;
        json!({"result": {"data": [r]}})
    };
    assert!(vectors(&row(json!(0.1)), 1).is_ok());
    assert!(vectors(&row(json!("x")), 1).is_err());
    assert!(vectors(&row(json!(1e300)), 1).is_err());
    assert!(vectors(&json!({"result": {"data": [vec![0.0; DIM]]}}), 1).is_err());
    assert!(vectors(&row(json!(0.1)), 2).is_err());
}

#[test]
fn bits_are_signs_msb_first() {
    let mut v = vec![-1.0f32; 16];
    v[0] = 0.5;
    v[9] = 0.1;
    assert_eq!(bits(&v), [0x80, 0x40]);
}
