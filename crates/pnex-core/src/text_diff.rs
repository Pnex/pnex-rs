//! Diff of two versions of a document by paragraph (partner-watch.md
//! D311): what was added, removed or changed, and where. Pure, wasm-safe.
//!
//! Common head and tail are cut first (edits are mostly local), then the
//! middle goes through a longest common subsequence over paragraphs,
//! bounded: past [`PARAGRAPHS_MAX`] paragraphs on a side the diff is cut
//! and marked `truncated`.

use serde::{Deserialize, Serialize};

/// Text read per side, bytes.
pub const TEXT_MAX: usize = 1024 * 1024;
/// Paragraphs compared per side after the common head and tail.
pub const PARAGRAPHS_MAX: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Added,
    Removed,
    Changed,
}

/// One change: paragraphs `from` (index in the old text) and `to` (index
/// in the new one), with their text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Hunk {
    pub change: Change,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TextDiff {
    #[cfg_attr(feature = "openapi", schema(format = Int64))]
    pub from_version: i64,
    #[cfg_attr(feature = "openapi", schema(format = Int64))]
    pub to_version: i64,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    pub hunks: Vec<Hunk>,
    /// A side was longer than the caps: the diff covers its start only.
    pub truncated: bool,
}

impl TextDiff {
    /// "+3 −1 ~2" style counters for a message.
    pub fn summary(&self) -> String {
        format!("+{} -{} ~{}", self.added, self.removed, self.changed)
    }
}

/// Paragraphs: blocks separated by blank lines, whitespace squashed.
pub fn paragraphs(text: &str) -> Vec<String> {
    text.split("\n\n")
        .flat_map(|block| {
            // A list or table keeps one paragraph per line.
            if block.lines().count() > 1
                && block
                    .lines()
                    .all(|l| l.trim_start().starts_with(['-', '*', '|', '#']))
            {
                block.lines().map(str::to_string).collect::<Vec<_>>()
            } else {
                vec![block.to_string()]
            }
        })
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect()
}

fn cap(text: &str) -> (&str, bool) {
    if text.len() <= TEXT_MAX {
        return (text, false);
    }
    let mut cut = TEXT_MAX;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (&text[..cut], true)
}

pub fn diff(old: &str, new: &str) -> TextDiff {
    let (old, t1) = cap(old);
    let (new, t2) = cap(new);
    let a = paragraphs(old);
    let b = paragraphs(new);
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let mut truncated = t1 || t2;
    let mut am = &a[head..a.len() - tail];
    let mut bm = &b[head..b.len() - tail];
    if am.len() > PARAGRAPHS_MAX || bm.len() > PARAGRAPHS_MAX {
        truncated = true;
        am = &am[..am.len().min(PARAGRAPHS_MAX)];
        bm = &bm[..bm.len().min(PARAGRAPHS_MAX)];
    }
    // LCS table over the middle.
    let (n, m) = (am.len(), bm.len());
    let mut t = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i * (m + 1) + j] = if am[i] == bm[j] {
                t[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                t[(i + 1) * (m + 1) + j].max(t[i * (m + 1) + j + 1])
            };
        }
    }
    // Edit script, then each run between equal paragraphs becomes hunks:
    // paired removals and additions are changes, the rest removed / added.
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut run_r, mut run_a): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    let flush = |r: &mut Vec<usize>, ad: &mut Vec<usize>, hunks: &mut Vec<Hunk>| {
        let k = r.len().min(ad.len());
        let text = |side: &[String], idx: &[usize]| idx.iter().map(|i| side[*i].clone()).collect();
        if k > 0 {
            hunks.push(Hunk {
                change: Change::Changed,
                from: Some(r[0] + head),
                to: Some(ad[0] + head),
                before: text(am, &r[..k]),
                after: text(bm, &ad[..k]),
            });
        }
        if r.len() > k {
            hunks.push(Hunk {
                change: Change::Removed,
                from: Some(r[k] + head),
                to: None,
                before: text(am, &r[k..]),
                after: Vec::new(),
            });
        }
        if ad.len() > k {
            hunks.push(Hunk {
                change: Change::Added,
                from: None,
                to: Some(ad[k] + head),
                before: Vec::new(),
                after: text(bm, &ad[k..]),
            });
        }
        r.clear();
        ad.clear();
    };
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && am[i] == bm[j] {
            flush(&mut run_r, &mut run_a, &mut hunks);
            i += 1;
            j += 1;
        } else if i < n && (j == m || t[(i + 1) * (m + 1) + j] >= t[i * (m + 1) + j + 1]) {
            run_r.push(i);
            i += 1;
        } else {
            run_a.push(j);
            j += 1;
        }
    }
    flush(&mut run_r, &mut run_a, &mut hunks);
    let count = |c: Change| hunks.iter().filter(|h| h.change == c).count();
    TextDiff {
        added: count(Change::Added),
        removed: count(Change::Removed),
        changed: count(Change::Changed),
        hunks,
        truncated,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_edits_are_found_with_their_positions() {
        let old = "Titre RNCP12345\n\nPrix 1200 €\n\nÉligible CPF\n\nContact";
        let new = "Titre RNCP12345\n\nPrix 1500 €\n\nContact\n\nNouveau : en ligne";
        let d = diff(old, new);
        assert_eq!((d.added, d.removed, d.changed), (1, 1, 1), "{d:?}");
        let changed = d
            .hunks
            .iter()
            .find(|h| h.change == Change::Changed)
            .unwrap();
        assert_eq!((changed.from, changed.to), (Some(1), Some(1)));
        assert_eq!(changed.before, vec!["Prix 1200 €"]);
        assert_eq!(changed.after, vec!["Prix 1500 €"]);
        let removed = d
            .hunks
            .iter()
            .find(|h| h.change == Change::Removed)
            .unwrap();
        assert_eq!(removed.before, vec!["Éligible CPF"]);
        let added = d.hunks.iter().find(|h| h.change == Change::Added).unwrap();
        assert_eq!(
            (added.to, added.after.clone()),
            (Some(3), vec!["Nouveau : en ligne".to_string()])
        );
        assert_eq!(d.summary(), "+1 -1 ~1");
    }

    #[test]
    fn same_text_no_hunk_and_lists_split_by_line() {
        assert!(diff("a\n\nb", "a\n\n  b ").hunks.is_empty());
        let d = diff("- un\n- deux", "- un\n- trois");
        assert_eq!(d.changed, 1);
        assert_eq!(paragraphs("# T\n- a\n- b").len(), 3);
    }
}
