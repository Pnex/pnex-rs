//! Recognition by voice prints first (voice-id.md §1 bis, D978–D985): on a
//! stored recording, short pieces of speech are matched against every ready
//! print of the org, then the pieces nobody claimed are grouped into
//! anonymous voices. Pure computation: embeddings come from the caller.

use crate::voice_id::{cosine, normalized, MIN_MARGIN};

/// Shortest piece worth an embedding, ms.
pub const PIECE_MIN_MS: u32 = 1_500;
/// Longest piece: short enough to hold one speaker most of the time.
pub const PIECE_MAX_MS: u32 = 3_000;
/// A doubtful piece between two pieces of one person, this close, joins them.
pub const BRIDGE_GAP_MS: u32 = 2_000;
/// Cosine similarity that joins two anonymous groups.
pub const UNKNOWN_JOIN: f32 = 0.5;
/// An anonymous group with less speech folds into its closest one.
pub const UNKNOWN_MIN_MS: u32 = 5_000;

/// Pieces of at most `PIECE_MAX_MS` covering the speech `turns`
/// (`(start_ms, end_ms)`), in order; a long turn is shared out evenly, a
/// turn under `PIECE_MIN_MS` stays one piece (too short for a reliable
/// print: give it no embedding, it takes the name of its diarization voice).
pub fn pieces(turns: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut sorted: Vec<(u32, u32)> = turns.iter().copied().filter(|(s, e)| e > s).collect();
    sorted.sort_unstable();
    let mut out = Vec::new();
    for (s, e) in sorted {
        let len = e - s;
        if len < PIECE_MIN_MS {
            out.push((s, e));
            continue;
        }
        let n = len.div_ceil(PIECE_MAX_MS);
        let step = len / n;
        for k in 0..n {
            let a = s + k * step;
            let b = if k + 1 == n { e } else { a + step };
            out.push((a, b));
        }
    }
    out
}

/// A ready print: its centroid and calibrated threshold (V5).
#[derive(Debug, Clone)]
pub struct Print {
    pub centroid: Vec<f32>,
    pub threshold: f32,
}

/// Who speaks in a piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    /// Index of the print recognized.
    Person(usize),
    /// Anonymous voice of this recording (1, 2…).
    Unknown(u32),
    /// No usable embedding.
    Nobody,
}

/// The print a piece matches: accepted by its threshold and ahead of the
/// second accepted one by `MIN_MARGIN` (V7), else `None`.
pub fn best_print(v: &[f32], prints: &[Print]) -> Option<usize> {
    let mut scored: Vec<(f32, usize)> = prints
        .iter()
        .enumerate()
        .filter(|(_, p)| p.centroid.len() == v.len())
        .map(|(i, p)| (cosine(v, &p.centroid), i))
        .filter(|(s, i)| *s >= prints[*i].threshold)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    match scored.as_slice() {
        [best, second, ..] if best.0 - second.0 < MIN_MARGIN => None,
        [best, ..] => Some(best.1),
        [] => None,
    }
}

/// One piece: its span, embedding (`None` when too short or the runtime
/// gave none) and the diarization voice it came from (window and local
/// speaker), if any.
#[derive(Debug, Clone)]
pub struct Piece {
    pub start_ms: u32,
    pub end_ms: u32,
    pub v: Option<Vec<f32>>,
    pub group: Option<u64>,
}

/// Who speaks in each piece: prints first, temporal smoothing, then the
/// pieces left are grouped into anonymous voices over the whole recording.
pub fn recognize(pieces: &[Piece], prints: &[Print]) -> Vec<Who> {
    let mut who: Vec<Option<usize>> = pieces
        .iter()
        .map(|p| p.v.as_deref().and_then(|v| best_print(v, prints)))
        .collect();
    smooth(pieces, &mut who);
    let anonymous = group_unknown(pieces, &who);
    let mut out: Vec<Who> = who
        .iter()
        .zip(anonymous)
        .map(|(w, a)| match (w, a) {
            (Some(i), _) => Who::Person(*i),
            (None, Some(k)) => Who::Unknown(k),
            _ => Who::Nobody,
        })
        .collect();
    fill_from_group(pieces, &mut out);
    out
}

/// A piece without a verdict (too short, no embedding) takes what most of
/// its diarization voice's speech got, else its nearest decided neighbour
/// within `BRIDGE_GAP_MS`.
fn fill_from_group(pieces: &[Piece], who: &mut [Who]) {
    let decided: Vec<Who> = who.to_vec();
    for i in 0..pieces.len() {
        if who[i] != Who::Nobody {
            continue;
        }
        let mut votes: Vec<(Who, u32)> = Vec::new();
        if let Some(g) = pieces[i].group {
            for (j, p) in pieces.iter().enumerate() {
                if p.group == Some(g) && decided[j] != Who::Nobody {
                    let len = p.end_ms - p.start_ms;
                    match votes.iter_mut().find(|(w, _)| *w == decided[j]) {
                        Some((_, n)) => *n += len,
                        None => votes.push((decided[j], len)),
                    }
                }
            }
        }
        if let Some((w, _)) = votes.iter().max_by_key(|(_, n)| *n) {
            who[i] = *w;
            continue;
        }
        let gap = |j: usize| {
            let (a, b) = (&pieces[i], &pieces[j]);
            a.start_ms
                .saturating_sub(b.end_ms)
                .max(b.start_ms.saturating_sub(a.end_ms))
        };
        if let Some(j) = (0..pieces.len())
            .filter(|&j| decided[j] != Who::Nobody && gap(j) <= BRIDGE_GAP_MS)
            .min_by_key(|&j| gap(j))
        {
            who[i] = decided[j];
        }
    }
}

/// A piece without a person, close on both sides to pieces of one same
/// person, joins them (a person does not change every two seconds).
fn smooth(pieces: &[Piece], who: &mut [Option<usize>]) {
    for i in 0..pieces.len() {
        if who[i].is_some() {
            continue;
        }
        let before = (0..i).rev().find(|&j| who[j].is_some());
        let after = (i + 1..pieces.len()).find(|&j| who[j].is_some());
        if let (Some(b), Some(a)) = (before, after) {
            let near = pieces[i].start_ms.saturating_sub(pieces[b].end_ms) <= BRIDGE_GAP_MS
                && pieces[a].start_ms.saturating_sub(pieces[i].end_ms) <= BRIDGE_GAP_MS;
            if near && who[b] == who[a] {
                who[i] = who[b];
            }
        }
    }
}

/// Anonymous voice number of each piece nobody claimed: agglomerative
/// clustering (centroid linkage, weighted by duration) while the closest
/// groups are at least `UNKNOWN_JOIN` similar and never overlap in time,
/// then groups under `UNKNOWN_MIN_MS` folded into their closest one.
/// Numbered by first appearance.
fn group_unknown(pieces: &[Piece], who: &[Option<usize>]) -> Vec<Option<u32>> {
    struct Group {
        sum: Vec<f32>,
        spans: Vec<(u32, u32)>,
        members: Vec<usize>,
        ms: u32,
    }
    let mut groups: Vec<Group> = pieces
        .iter()
        .enumerate()
        .filter(|(i, _)| who[*i].is_none())
        .filter_map(|(i, p)| {
            let v = normalized(p.v.as_deref()?)?;
            let w = (p.end_ms - p.start_ms) as f32;
            Some(Group {
                sum: v.iter().map(|x| x * w).collect(),
                spans: vec![(p.start_ms, p.end_ms)],
                members: vec![i],
                ms: p.end_ms - p.start_ms,
            })
        })
        .collect();
    let apart = |a: &Group, b: &Group| {
        a.spans
            .iter()
            .all(|x| b.spans.iter().all(|y| x.1 <= y.0 || y.1 <= x.0))
    };
    let sim = |a: &Group, b: &Group| match (normalized(&a.sum), normalized(&b.sum)) {
        (Some(x), Some(y)) if x.len() == y.len() => cosine(&x, &y),
        _ => f32::MIN,
    };
    let merge = |groups: &mut Vec<Group>, i: usize, j: usize| {
        let b = groups.swap_remove(i.max(j));
        let a = &mut groups[i.min(j)];
        a.sum.iter_mut().zip(&b.sum).for_each(|(x, y)| *x += y);
        a.spans.extend(b.spans);
        a.members.extend(b.members);
        a.ms += b.ms;
    };
    // shortcut: O(n³) pair search, fine for the unclaimed pieces of a few
    // hours; a priority queue beyond.
    loop {
        let mut best: Option<(f32, usize, usize)> = None;
        for i in 0..groups.len() {
            for j in i + 1..groups.len() {
                if !apart(&groups[i], &groups[j]) {
                    continue;
                }
                let s = sim(&groups[i], &groups[j]);
                if s >= UNKNOWN_JOIN && best.is_none_or(|b| s > b.0) {
                    best = Some((s, i, j));
                }
            }
        }
        let Some((_, i, j)) = best else { break };
        merge(&mut groups, i, j);
    }
    while let Some(small) = (0..groups.len())
        .filter(|&i| groups[i].ms < UNKNOWN_MIN_MS)
        .min_by_key(|&i| groups[i].ms)
    {
        let target = (0..groups.len())
            .filter(|&j| j != small && groups[j].ms >= UNKNOWN_MIN_MS)
            .filter(|&j| apart(&groups[small], &groups[j]))
            .max_by(|&a, &b| {
                sim(&groups[small], &groups[a]).total_cmp(&sim(&groups[small], &groups[b]))
            });
        match target {
            Some(j) => merge(&mut groups, small, j),
            // Nowhere to go: it stays a voice of its own.
            None => groups[small].ms = UNKNOWN_MIN_MS,
        }
    }
    groups.sort_by_key(|g| g.spans.iter().map(|s| s.0).min());
    let mut out = vec![None; pieces.len()];
    for (k, g) in groups.iter().enumerate() {
        for &m in &g.members {
            out[m] = Some(k as u32 + 1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_become_short_pieces() {
        assert_eq!(pieces(&[(0, 1_000)]), vec![(0, 1_000)], "short: one piece");
        assert_eq!(pieces(&[(0, 2_000)]), vec![(0, 2_000)]);
        assert_eq!(
            pieces(&[(0, 7_000)]),
            vec![(0, 2_333), (2_333, 4_666), (4_666, 7_000)]
        );
        assert!(pieces(&[(10_000, 19_000), (0, 4_000)])
            .iter()
            .all(|(s, e)| e - s >= PIECE_MIN_MS && e - s <= PIECE_MAX_MS));
        assert!(pieces(&[(0, 900), (5_000, 6_000)])
            .iter()
            .all(|(s, e)| e - s <= 1_000));
    }

    fn print(v: &[f32]) -> Print {
        Print {
            centroid: v.to_vec(),
            threshold: 0.6,
        }
    }

    fn piece(s: u32, e: u32, v: &[f32]) -> Piece {
        Piece {
            start_ms: s,
            end_ms: e,
            v: Some(v.to_vec()),
            group: None,
        }
    }

    #[test]
    fn a_piece_goes_to_the_clear_best_print_only() {
        let prints = [print(&[1.0, 0.0, 0.0]), print(&[0.0, 1.0, 0.0])];
        assert_eq!(best_print(&[0.9, 0.1, 0.0], &prints), Some(0));
        assert_eq!(best_print(&[0.0, 0.0, 1.0], &prints), None, "nobody");
        assert_eq!(
            best_print(&[0.7, 0.7, 0.0], &prints),
            None,
            "too close to call"
        );
    }

    #[test]
    fn many_people_are_told_apart_and_unknowns_grouped() {
        // Six known people take turns; two strangers speak too.
        let dims = 8;
        let unit = |k: usize| {
            let mut v = vec![0.0; dims];
            v[k] = 1.0;
            v
        };
        let prints: Vec<Print> = (0..6).map(|k| print(&unit(k))).collect();
        let mut ps = Vec::new();
        let mut t = 0;
        for round in 0..3 {
            for k in 0..8 {
                ps.push(piece(t, t + 3_000, &unit(k)));
                t += 3_000;
            }
            let _ = round;
        }
        let who = recognize(&ps, &prints);
        for (i, w) in who.iter().enumerate() {
            match i % 8 {
                k @ 0..=5 => assert_eq!(*w, Who::Person(k), "piece {i}"),
                6 => assert_eq!(*w, Who::Unknown(1), "piece {i}"),
                _ => assert_eq!(*w, Who::Unknown(2), "piece {i}"),
            }
        }
    }

    #[test]
    fn a_doubtful_piece_between_one_person_joins_them() {
        let prints = [print(&[1.0, 0.0])];
        let ps = [
            piece(0, 3_000, &[1.0, 0.0]),
            piece(3_500, 6_000, &[0.5, 0.5]),
            piece(6_500, 9_000, &[1.0, 0.0]),
            Piece {
                start_ms: 20_000,
                end_ms: 22_000,
                v: None,
                group: None,
            },
        ];
        let who = recognize(&ps, &prints);
        assert_eq!(who[1], Who::Person(0));
        assert_eq!(who[3], Who::Nobody, "alone and far");
    }

    #[test]
    fn a_short_piece_takes_the_name_of_its_diarization_voice() {
        let prints = [print(&[1.0, 0.0]), print(&[0.0, 1.0])];
        let short = |s: u32, g: u64| Piece {
            start_ms: s,
            end_ms: s + 800,
            v: None,
            group: Some(g),
        };
        let mut a = piece(0, 3_000, &[1.0, 0.0]);
        a.group = Some(1);
        let mut b = piece(10_000, 13_000, &[0.0, 1.0]);
        b.group = Some(2);
        // Short pieces far from anything: only their group names them.
        let ps = [a, b, short(30_000, 1), short(40_000, 2), short(50_000, 9)];
        let who = recognize(&ps, &prints);
        assert_eq!(who[2], Who::Person(0));
        assert_eq!(who[3], Who::Person(1));
        assert_eq!(who[4], Who::Nobody);
    }
}
