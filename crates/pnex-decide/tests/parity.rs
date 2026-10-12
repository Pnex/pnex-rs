//! Parity with the reference implementation (Q12): token ids, marker
//! positions and logits of `ref.json` (ONNX Runtime + the Python port of
//! the sequence layout) must match this crate. Needs a bundle directory in
//! `PNEX_LAYA_DIR` holding `ref.json`: `--ignored`.

use pnex_decide::{Laya, Question};

#[test]
#[ignore = "requires a Laya bundle + ref.json in PNEX_LAYA_DIR"]
fn logits_match_the_reference() {
    let dir = std::path::PathBuf::from(std::env::var("PNEX_LAYA_DIR").expect("PNEX_LAYA_DIR"));
    let started = std::time::Instant::now();
    let laya = Laya::load(&dir).expect("load");
    eprintln!("load + optimize: {} ms", started.elapsed().as_millis());
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("ref.json")).unwrap()).unwrap();
    for case in cases {
        let rq = &case["question"];
        let options: Vec<String> = match &rq["criteria"] {
            serde_json::Value::Object(o) => o.keys().cloned().collect(),
            other => serde_json::from_value(other.clone()).unwrap(),
        };
        let descriptions: Vec<String> = match &rq["criteria"] {
            serde_json::Value::Object(o) => o
                .values()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect(),
            _ => vec![],
        };
        let q = Question {
            kind: serde_json::from_value(rq["type"].clone()).unwrap(),
            instructions: rq["instructions"].as_str().unwrap().into(),
            options,
            descriptions,
            ..Default::default()
        };
        let (ids, markers) = laya.sequence(&case["state"], &q);
        let want_ids: Vec<u32> = serde_json::from_value(case["ids"].clone()).unwrap();
        let want_markers: Vec<usize> = serde_json::from_value(case["markers"].clone()).unwrap();
        // Object states: serde_json sorts keys, Python keeps insertion
        // order (documented shortcut) — the graph is still checked on the
        // reference ids.
        let (ids, markers) = if case["state"].is_string() {
            assert_eq!(ids, want_ids, "token ids");
            assert_eq!(markers, want_markers, "markers");
            (ids, markers)
        } else {
            (want_ids, want_markers)
        };
        let t = std::time::Instant::now();
        let got = laya
            .logits(&[(ids, markers, q.kind)])
            .expect("run")
            .remove(0);
        let want: Vec<f32> = serde_json::from_value(case["logits"].clone()).unwrap();
        let diff = got
            .iter()
            .zip(&want)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        eprintln!(
            "{}: {} ms (ORT {} ms), max |Δlogit| = {diff:e}",
            rq["type"],
            t.elapsed().as_millis(),
            case["ms"]
        );
        assert!(diff < 1e-3, "logits {got:?} vs {want:?}");
    }
}

/// Full `decide` path on a real bundle: one batch, calibrated
/// distributions that sum to 1, labels in option order.
#[test]
#[ignore = "requires a Laya bundle in PNEX_LAYA_DIR"]
fn decide_answers_in_one_batch() {
    use pnex_decide::QuestionKind;
    let dir = std::path::PathBuf::from(std::env::var("PNEX_LAYA_DIR").expect("PNEX_LAYA_DIR"));
    let laya = Laya::load(&dir).expect("load");
    let q = |kind, options: &[&str]| Question {
        kind,
        instructions: "Does this describe an incident?".into(),
        options: options.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    let state = serde_json::json!(
        "Un chariot élévateur heurte une personne sur le quai 2, elle crie à l'aide."
    );
    let (answers, ms) = laya
        .decide(
            &state,
            &[
                (
                    "incident".into(),
                    q(QuestionKind::Choice, &["incident", "routine"]),
                ),
                (
                    "urgency".into(),
                    q(QuestionKind::Score, &["none", "soon", "now"]),
                ),
            ],
        )
        .expect("decide");
    eprintln!("{answers:?} in {ms} ms");
    assert_eq!(answers.len(), 2);
    for (_, a) in &answers {
        let sum: f32 = a.probabilities.iter().sum();
        assert!((sum - 1.0).abs() < 1e-3, "{sum}");
    }
    assert!(answers[1].1.expected.is_some());
}
