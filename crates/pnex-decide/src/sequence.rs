//! Request rendering of Laya, ported from `@receptron/laya` `sequence.ts`
//! (MIT), itself a port of the checkpoint's `rl_common.py`: option texts,
//! sequence layout, per-cardinality temperature, confidence. Model-free,
//! unit-tested without the weights.

pub use pnex_core::decide::{DecideQuestion as Question, QuestionKind};

/// Option texts in label-index order (`renderOptions`).
pub fn render_options(q: &Question) -> Vec<String> {
    match q.kind {
        QuestionKind::Choice => q
            .options
            .iter()
            .enumerate()
            .map(
                |(i, o)| match q.descriptions.get(i).filter(|d| !d.is_empty()) {
                    Some(d) => format!("{o}: {d}"),
                    None => o.clone(),
                },
            )
            .collect(),
        QuestionKind::Score => q
            .options
            .iter()
            .enumerate()
            .map(|(i, c)| format!("level {i}: {c}"))
            .collect(),
        QuestionKind::Noul => vec![
            "false: no, the statement does not hold".into(),
            "true: yes, the statement holds".into(),
        ],
    }
}

/// Python's `json.dumps(v, ensure_ascii=False)`: `", "` / `": "` separators.
/// shortcut: object keys follow serde_json's map order and floats Rust's
/// shortest form (Python writes `1e-05`); states are plain text in V1.
pub fn py_json_dumps(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(py_json_dumps).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, x)| format!(
                    "{}: {}",
                    serde_json::to_string(k).unwrap_or_default(),
                    py_json_dumps(x)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Text of a state: a string as is, anything else as Python JSON.
pub fn serialize_state(state: &serde_json::Value) -> String {
    match state {
        serde_json::Value::String(s) => s.clone(),
        other => py_json_dumps(other),
    }
}

/// Special token ids of the tokenizer.
#[derive(Debug, Clone, Copy)]
pub struct SpecialIds {
    pub cls: u32,
    pub sep: u32,
    pub mask: u32,
    pub pad: u32,
}

/// `[CLS] <kind> question: instructions [SEP] [MASK] opt0 [MASK] opt1 …
/// [SEP] state [SEP]` → `(ids, marker positions)`.
pub fn build_sequence(
    encode: &dyn Fn(&str) -> Vec<u32>,
    ids: SpecialIds,
    mask_token: &str,
    state: &serde_json::Value,
    q: &Question,
    max_len: usize,
    head_max_len: usize,
) -> (Vec<u32>, Vec<usize>) {
    // The mask literal is scrubbed from user text: no marker injection.
    let scrub = |s: &str| s.split(mask_token).collect::<Vec<_>>().join(" ");
    let kind = match q.kind {
        QuestionKind::Choice => "choice",
        QuestionKind::Score => "score",
        QuestionKind::Noul => "noul",
    };
    let mut head = encode(&format!("{kind} question: {}", scrub(&q.instructions)));
    let mut opts: Vec<Vec<u32>> = render_options(q)
        .iter()
        .map(|o| {
            let mut v = vec![ids.mask];
            v.extend(encode(&format!(" {}", scrub(o))).into_iter().take(48));
            v
        })
        .collect();
    let total = |xs: &[Vec<u32>]| xs.iter().map(Vec::len).sum::<usize>();
    let mut budget = head_max_len as isize - total(&opts) as isize;
    if budget < 16 {
        let per = ((head_max_len as isize - 16) / opts.len().max(1) as isize).max(4) as usize;
        for o in &mut opts {
            o.truncate(per);
        }
        budget = head_max_len as isize - total(&opts) as isize;
    }
    head.truncate(budget.max(8) as usize);
    let mut seq = vec![ids.cls];
    seq.extend(head);
    seq.push(ids.sep);
    let mut markers = Vec::with_capacity(opts.len());
    for o in opts {
        markers.push(seq.len());
        seq.extend(o);
    }
    seq.push(ids.sep);
    let room = max_len.saturating_sub(seq.len() + 1);
    seq.extend(
        encode(&scrub(&serialize_state(state)))
            .into_iter()
            .take(room),
    );
    seq.push(ids.sep);
    seq.truncate(max_len);
    markers.retain(|m| *m < max_len);
    (seq, markers)
}

fn size_bucket(k: usize) -> &'static str {
    match k {
        0..=2 => "2",
        3..=5 => "3-5",
        6..=10 => "6-10",
        _ => "11+",
    }
}

/// Key of the per-cardinality temperature (`choice:3-5`…).
pub fn temp_bucket(kind: QuestionKind, k: usize) -> String {
    let name = match kind {
        QuestionKind::Choice => "choice",
        QuestionKind::Score => "score",
        QuestionKind::Noul => "noul",
    };
    format!("{name}:{}", size_bucket(k))
}

pub fn softmax(z: &[f32]) -> Vec<f32> {
    let max = z.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let e: Vec<f32> = z.iter().map(|v| (v - max).exp()).collect();
    let sum: f32 = e.iter().sum();
    e.iter().map(|v| v / sum).collect()
}

/// 1 − normalized entropy of the distribution.
pub fn confidence(p: &[f32]) -> f32 {
    let k = p.len();
    if k < 2 {
        return 1.0;
    }
    let ent: f32 = p.iter().map(|x| -x * x.max(1e-12).ln()).sum();
    1.0 - ent / (k as f32).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_json() {
        let q = Question {
            kind: QuestionKind::Choice,
            instructions: "x".into(),
            options: vec!["incident".into(), "routine".into()],
            descriptions: vec!["someone at risk".into()],
            ..Default::default()
        };
        assert_eq!(
            render_options(&q),
            vec!["incident: someone at risk", "routine"]
        );
        let s = Question {
            kind: QuestionKind::Score,
            options: vec!["low".into(), "high".into()],
            ..q
        };
        assert_eq!(render_options(&s), vec!["level 0: low", "level 1: high"]);
        assert_eq!(
            py_json_dumps(&serde_json::json!({"a": [1, "é"], "b": null})),
            "{\"a\": [1, \"é\"], \"b\": null}"
        );
    }

    #[test]
    fn layout_and_mask_scrub() {
        // One id per character, so lengths are easy to follow.
        let encode = |t: &str| t.chars().map(|c| c as u32 + 100).collect::<Vec<_>>();
        let ids = SpecialIds {
            cls: 1,
            sep: 2,
            mask: 3,
            pad: 0,
        };
        let q = Question {
            kind: QuestionKind::Choice,
            instructions: "a[MASK]b".into(),
            options: vec!["x".into(), "y".into()],
            ..Default::default()
        };
        let (seq, markers) = build_sequence(
            &encode,
            ids,
            "[MASK]",
            &serde_json::json!("st"),
            &q,
            512,
            192,
        );
        assert_eq!(seq[0], 1);
        assert_eq!(markers.len(), 2);
        assert!(markers.iter().all(|m| seq[*m] == 3));
        assert_eq!(
            seq.iter().filter(|v| **v == 3).count(),
            2,
            "user [MASK] scrubbed"
        );
        assert_eq!(*seq.last().unwrap(), 2);
        let (seq, _) = build_sequence(
            &encode,
            ids,
            "[MASK]",
            &serde_json::json!("s".repeat(900)),
            &q,
            64,
            192,
        );
        assert_eq!(seq.len(), 64);
    }

    #[test]
    fn calibration_helpers() {
        assert_eq!(temp_bucket(QuestionKind::Choice, 4), "choice:3-5");
        let p = softmax(&[1.0, 1.0]);
        assert!((p[0] - 0.5).abs() < 1e-6);
        assert!(confidence(&p).abs() < 1e-6);
        assert!((confidence(&[1.0, 0.0]) - 1.0).abs() < 1e-4);
    }
}
