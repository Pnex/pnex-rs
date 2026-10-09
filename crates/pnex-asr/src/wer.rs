//! Word error rate for model evaluation (media-ingest.md §9, D167 check).

/// Lowercases, unifies apostrophes and keeps letters, digits and
/// in-word apostrophes; everything else separates words. Hyphens split
/// (« peut-être » → « peut être ») so both spellings score the same.
pub fn normalize(text: &str) -> Vec<String> {
    let lowered = text
        .to_lowercase()
        .replace(['\u{2019}', '\u{2018}', '`'], "'");
    let cleaned: String = lowered
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect();
    cleaned
        .split_whitespace()
        .map(|w| w.trim_matches('\'').to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Levenshtein distance over words: substitutions + deletions + insertions.
pub fn edit_distance(reference: &[String], hypothesis: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=hypothesis.len()).collect();
    let mut cur = vec![0; hypothesis.len() + 1];
    for (i, r) in reference.iter().enumerate() {
        cur[0] = i + 1;
        for (j, h) in hypothesis.iter().enumerate() {
            let sub = prev[j] + usize::from(r != h);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[hypothesis.len()]
}

/// Accumulates a corpus-level WER (sum of edits / sum of reference words).
#[derive(Debug, Default, Clone, Copy)]
pub struct WerAccumulator {
    pub edits: usize,
    pub ref_words: usize,
}

impl WerAccumulator {
    pub fn add(&mut self, reference: &str, hypothesis: &str) {
        let r = normalize(reference);
        let h = normalize(hypothesis);
        self.edits += edit_distance(&r, &h);
        self.ref_words += r.len();
    }

    pub fn wer(&self) -> f64 {
        if self.ref_words == 0 {
            0.0
        } else {
            self.edits as f64 / self.ref_words as f64
        }
    }
}

/// Proper nouns of a raw (cased) reference: capitalized words that are not
/// the first word of the sentence. Rough, but stable across models.
pub fn proper_nouns(raw_reference: &str) -> Vec<String> {
    raw_reference
        .split_whitespace()
        .skip(1)
        .filter(|w| w.chars().next().is_some_and(char::is_uppercase))
        .flat_map(normalize)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_ignores_case_punctuation_and_apostrophe_style() {
        assert_eq!(
            normalize("L’homme, peut-être !"),
            ["l'homme", "peut", "être"]
        );
    }

    #[test]
    fn distance_counts_each_edit_kind() {
        let r = normalize("le chat dort ici");
        assert_eq!(edit_distance(&r, &normalize("le chat dort ici")), 0);
        assert_eq!(edit_distance(&r, &normalize("le chien dort ici")), 1);
        assert_eq!(edit_distance(&r, &normalize("le chat dort")), 1);
        assert_eq!(edit_distance(&r, &normalize("le gros chat dort ici")), 1);
    }

    #[test]
    fn proper_nouns_skip_the_sentence_start() {
        assert_eq!(
            proper_nouns("Le président Macron est à Lyon."),
            ["macron", "lyon"]
        );
    }
}
