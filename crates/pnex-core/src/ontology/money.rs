//! Money flows of a case (cases-money.md K54–K57): payments extracted from
//! the case events, and the Sankey layout of the Money view. Deterministic
//! and wasm32-safe: the same input always draws the same diagram.
//!
//! A payment is an event with an `amount` and `payer` / `payee`
//! attachments. Amounts add up per currency only, never converted.

use std::collections::{BTreeMap, BTreeSet};

use super::api::CaseWorkspace;
use super::schema::Money;
use super::{EVENT_AMOUNT, EVENT_OCCURRED, ROLE_PAYEE, ROLE_PAYER};

/// Stand-in for the missing end of a payment whose payer or payee is not
/// attached yet: the money is shown coming from, or going to, "unknown".
pub const UNKNOWN_PAYER: &str = "@unknown-payer";
pub const UNKNOWN_PAYEE: &str = "@unknown-payee";

/// One payment, or one share of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Transfer {
    pub event_id: String,
    pub from: String,
    pub to: String,
    pub amount: f64,
    pub currency: String,
    pub estimated: bool,
    /// A piece is attached to the event as evidence or source (K52).
    pub sourced: bool,
    /// Start of the event's `occurred` (RFC 3339 UTC), for the time cursor.
    pub at: Option<String>,
}

/// Payments of a case. An event with several payers or payees is split
/// evenly between the pairs and marked estimated.
// shortcut: even split of a multi-party payment, upgrade when an
// attachment can carry its own share of the amount.
pub fn transfers(ws: &CaseWorkspace) -> Vec<Transfer> {
    let mut out = Vec::new();
    for ev in &ws.events {
        let Some(m) = ev.event.properties.get(EVENT_AMOUNT).and_then(Money::parse) else {
            continue;
        };
        if m.amount <= 0.0 {
            continue;
        }
        let ends = |role: &str, unknown: &str| {
            let v: Vec<String> = ev
                .attachments
                .iter()
                .filter(|a| a.role == role)
                .map(|a| a.object.id.clone())
                .collect();
            if v.is_empty() {
                vec![unknown.to_string()]
            } else {
                v
            }
        };
        let (payers, payees) = (
            ends(ROLE_PAYER, UNKNOWN_PAYER),
            ends(ROLE_PAYEE, UNKNOWN_PAYEE),
        );
        let pairs = (payers.len() * payees.len()) as f64;
        let sourced = ev
            .attachments
            .iter()
            .any(|a| matches!(a.role.as_str(), "evidence" | "source"));
        let at = ev
            .event
            .properties
            .get(EVENT_OCCURRED)
            .and_then(|o| o.get("from"))
            .and_then(|f| f.as_str())
            .map(str::to_string);
        for p in &payers {
            for q in &payees {
                out.push(Transfer {
                    event_id: ev.event.id.clone(),
                    from: p.clone(),
                    to: q.clone(),
                    amount: m.amount / pairs,
                    currency: m.currency.clone(),
                    estimated: m.estimated || pairs > 1.0,
                    sourced,
                    at: at.clone(),
                });
            }
        }
    }
    out
}

/// Currencies by total amount, largest first: the view shows one at a time.
pub fn currencies(ts: &[Transfer]) -> Vec<String> {
    let mut sums: BTreeMap<&str, f64> = BTreeMap::new();
    for t in ts {
        *sums.entry(&t.currency).or_default() += t.amount;
    }
    let mut v: Vec<(&str, f64)> = sums.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    v.into_iter().map(|(c, _)| c.to_string()).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Object,
    UnknownPayer,
    UnknownPayee,
    /// Money received by an intermediary and not traced further (K57).
    Untraced,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SankeyNode {
    /// Object id; for `Untraced`, the id of the intermediary.
    pub id: String,
    pub kind: NodeKind,
    pub col: usize,
    /// Top and height, in layout units (`0..height`).
    pub y: f64,
    pub h: f64,
    pub value_in: f64,
    pub value_out: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SankeyLink {
    pub from: usize,
    pub to: usize,
    pub value: f64,
    pub estimated: bool,
    /// One of its payments has no evidence attached (K52).
    pub unsourced: bool,
    /// Goes back to a column at or before its source: part of a cycle.
    pub back: bool,
    /// Payment events behind the band (empty for an untraced residual).
    pub events: Vec<String>,
    /// Band top at the source and at the target, and its width.
    pub y0: f64,
    pub y1: f64,
    pub w: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sankey {
    pub nodes: Vec<SankeyNode>,
    pub links: Vec<SankeyLink>,
    pub cols: usize,
}

/// A residual under this share of the inflow is rounding, not a gap.
const RESIDUAL_MIN: f64 = 0.005;
const SWEEPS: usize = 6;

/// Aggregate of a payer → payee pair: amount, estimated, unsourced, events.
type Pair = (f64, bool, bool, BTreeSet<String>);

/// Sankey layout of transfers in one currency, in a `height` tall area with
/// `pad` between stacked nodes.
pub fn layout(ts: &[Transfer], height: f64, pad: f64) -> Sankey {
    // 1. Aggregate per ordered pair; self payments carry no flow.
    let mut pairs: BTreeMap<(String, String), Pair> = BTreeMap::new();
    for t in ts.iter().filter(|t| t.from != t.to && t.amount > 0.0) {
        let e = pairs.entry((t.from.clone(), t.to.clone())).or_default();
        e.0 += t.amount;
        e.1 |= t.estimated;
        e.2 |= !t.sourced;
        e.3.insert(t.event_id.clone());
    }
    if pairs.is_empty() {
        return Sankey::default();
    }
    let ids: Vec<String> = pairs
        .keys()
        .flat_map(|(a, b)| [a.clone(), b.clone()])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let index: BTreeMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    let kind_of = |id: &str| match id {
        UNKNOWN_PAYER => NodeKind::UnknownPayer,
        UNKNOWN_PAYEE => NodeKind::UnknownPayee,
        _ => NodeKind::Object,
    };
    let mut nodes: Vec<SankeyNode> = ids
        .iter()
        .map(|id| SankeyNode {
            id: id.clone(),
            kind: kind_of(id),
            col: 0,
            y: 0.0,
            h: 0.0,
            value_in: 0.0,
            value_out: 0.0,
        })
        .collect();
    let mut links: Vec<SankeyLink> = pairs
        .into_iter()
        .map(
            |((a, b), (value, estimated, unsourced, events))| SankeyLink {
                from: index[a.as_str()],
                to: index[b.as_str()],
                value,
                estimated,
                unsourced,
                back: false,
                events: events.into_iter().collect(),
                y0: 0.0,
                y1: 0.0,
                w: 0.0,
            },
        )
        .collect();

    // 2. Cycles: depth-first, an edge to a node on the stack is a back edge.
    let n = nodes.len();
    let mut out_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, l) in links.iter().enumerate() {
        out_edges[l.from].push(i);
    }
    let mut state = vec![0u8; n]; // 0 new, 1 on stack, 2 done
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![(root, 0usize)];
        state[root] = 1;
        while let Some(&(v, next)) = stack.last() {
            if let Some(&li) = out_edges[v].get(next) {
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
                let w = links[li].to;
                match state[w] {
                    0 => {
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => links[li].back = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }

    // 3. Columns: longest path over forward edges, then sinks to the right.
    let mut indeg = vec![0usize; n];
    for l in links.iter().filter(|l| !l.back) {
        indeg[l.to] += 1;
    }
    let mut queue: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    while let Some(v) = queue.pop() {
        for &li in &out_edges[v] {
            let l = &links[li];
            if l.back {
                continue;
            }
            nodes[l.to].col = nodes[l.to].col.max(nodes[v].col + 1);
            indeg[l.to] -= 1;
            if indeg[l.to] == 0 {
                queue.push(l.to);
            }
        }
    }
    let last = nodes.iter().map(|x| x.col).max().unwrap_or(0);
    for (i, node) in nodes.iter_mut().enumerate() {
        if !links.iter().any(|l| l.from == i && !l.back) {
            node.col = last;
        }
    }
    for l in &links {
        nodes[l.from].value_out += l.value;
        nodes[l.to].value_in += l.value;
    }

    // 4. Untraced residual of every intermediary.
    for i in 0..n {
        let (vin, vout) = (nodes[i].value_in, nodes[i].value_out);
        if vout > 0.0 && vin - vout > RESIDUAL_MIN * vin {
            let col = nodes[i].col + 1;
            nodes.push(SankeyNode {
                id: nodes[i].id.clone(),
                kind: NodeKind::Untraced,
                col,
                y: 0.0,
                h: 0.0,
                value_in: vin - vout,
                value_out: 0.0,
            });
            nodes[i].value_out = vin;
            links.push(SankeyLink {
                from: i,
                to: nodes.len() - 1,
                value: vin - vout,
                estimated: false,
                unsourced: false,
                back: false,
                events: Vec::new(),
                y0: 0.0,
                y1: 0.0,
                w: 0.0,
            });
        }
    }
    let cols = nodes.iter().map(|x| x.col).max().unwrap_or(0) + 1;

    // 5. Vertical scale: the fullest column fills the height.
    let value = |x: &SankeyNode| x.value_in.max(x.value_out);
    let mut by_col: Vec<Vec<usize>> = vec![Vec::new(); cols];
    for (i, x) in nodes.iter().enumerate() {
        by_col[x.col].push(i);
    }
    let k = by_col
        .iter()
        .filter(|c| !c.is_empty())
        .map(|c| {
            let total: f64 = c.iter().map(|&i| value(&nodes[i])).sum();
            (height - pad * (c.len() - 1) as f64).max(height * 0.2) / total
        })
        .fold(f64::INFINITY, f64::min);
    for x in nodes.iter_mut() {
        x.h = value(x) * k;
    }

    // 6. Order within columns: biggest first, then barycentre sweeps.
    for c in by_col.iter_mut() {
        c.sort_by(|&a, &b| {
            value(&nodes[b])
                .total_cmp(&value(&nodes[a]))
                .then(a.cmp(&b))
        });
    }
    let place = |nodes: &mut Vec<SankeyNode>, by_col: &Vec<Vec<usize>>| {
        for c in by_col {
            let used: f64 =
                c.iter().map(|&i| nodes[i].h).sum::<f64>() + pad * c.len().saturating_sub(1) as f64;
            let mut y = ((height - used) / 2.0).max(0.0);
            for &i in c {
                nodes[i].y = y;
                y += nodes[i].h + pad;
            }
        }
    };
    place(&mut nodes, &by_col);
    for sweep in 0..SWEEPS {
        let forward = sweep % 2 == 0;
        let order: Vec<usize> = if forward {
            (1..cols).collect()
        } else {
            (0..cols.saturating_sub(1)).rev().collect()
        };
        for c in order {
            let centre = |i: usize| {
                let (mut sum, mut w) = (0.0, 0.0);
                for l in links.iter().filter(|l| !l.back) {
                    let other = match (forward, l.from == i, l.to == i) {
                        (true, _, true) => l.from,
                        (false, true, _) => l.to,
                        _ => continue,
                    };
                    sum += (nodes[other].y + nodes[other].h / 2.0) * l.value;
                    w += l.value;
                }
                if w > 0.0 {
                    sum / w
                } else {
                    nodes[i].y + nodes[i].h / 2.0
                }
            };
            let mut keyed: Vec<(f64, usize)> = by_col[c].iter().map(|&i| (centre(i), i)).collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            by_col[c] = keyed.into_iter().map(|(_, i)| i).collect();
        }
        place(&mut nodes, &by_col);
    }

    // 7. Band offsets: stacked at each end in the order of the other end.
    for l in links.iter_mut() {
        l.w = l.value * k;
    }
    let mid = |nodes: &Vec<SankeyNode>, i: usize| nodes[i].y + nodes[i].h / 2.0;
    for i in 0..nodes.len() {
        let mut outs: Vec<usize> = (0..links.len()).filter(|&j| links[j].from == i).collect();
        outs.sort_by(|&a, &b| {
            (links[a].back, mid(&nodes, links[a].to))
                .partial_cmp(&(links[b].back, mid(&nodes, links[b].to)))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(&b))
        });
        let mut y = nodes[i].y;
        for j in outs {
            links[j].y0 = y;
            y += links[j].w;
        }
        let mut ins: Vec<usize> = (0..links.len()).filter(|&j| links[j].to == i).collect();
        ins.sort_by(|&a, &b| {
            (links[a].back, mid(&nodes, links[a].from))
                .partial_cmp(&(links[b].back, mid(&nodes, links[b].from)))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(&b))
        });
        let mut y = nodes[i].y;
        for j in ins {
            links[j].y1 = y;
            y += links[j].w;
        }
    }
    Sankey { nodes, links, cols }
}

/// Compact amount for labels: `1,2 M`, `340 k`, `12 500` (French) or
/// `1.2 M`, `340 k`, `12,500` (English).
pub fn compact_amount(amount: f64, french: bool) -> String {
    let (div, suffix) = match amount.abs() {
        a if a >= 1e9 => (1e9, if french { " Md" } else { " bn" }),
        a if a >= 1e6 => (1e6, " M"),
        a if a >= 1e5 => (1e3, " k"),
        _ => return full_amount(amount, french),
    };
    let v = amount / div;
    let s = if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}").trim_end_matches(".0").to_string()
    };
    let s = if french { s.replace('.', ",") } else { s };
    format!("{s}{suffix}")
}

/// Whole amount with thousands grouped, cents only when there are some.
pub fn full_amount(amount: f64, french: bool) -> String {
    let cents = (amount.abs() * 100.0).round() as u128;
    let (int, frac) = (cents / 100, cents % 100);
    let digits = int.to_string();
    let sep = if french { '\u{202f}' } else { ',' };
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(sep);
        }
        grouped.push(ch);
    }
    let sign = if amount < 0.0 && cents > 0 { "-" } else { "" };
    if frac == 0 {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}{}{frac:02}", if french { ',' } else { '.' })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(from: &str, to: &str, amount: f64, ev: &str) -> Transfer {
        Transfer {
            event_id: ev.into(),
            from: from.into(),
            to: to.into(),
            amount,
            currency: "EUR".into(),
            estimated: false,
            sourced: true,
            at: None,
        }
    }

    fn node<'a>(s: &'a Sankey, id: &str, kind: NodeKind) -> &'a SankeyNode {
        s.nodes
            .iter()
            .find(|n| n.id == id && n.kind == kind)
            .unwrap()
    }

    #[test]
    fn chain_with_untraced_residual() {
        // State allocates 100 to a programme, which pays 60 to two
        // associations: 40 is not traced further.
        let ts = [
            t("state", "prog", 100.0, "e1"),
            t("prog", "a1", 40.0, "e2"),
            t("prog", "a2", 20.0, "e3"),
        ];
        let s = layout(&ts, 300.0, 10.0);
        assert_eq!(s.cols, 3);
        assert_eq!(node(&s, "state", NodeKind::Object).col, 0);
        assert_eq!(node(&s, "prog", NodeKind::Object).col, 1);
        assert_eq!(node(&s, "a1", NodeKind::Object).col, 2);
        let u = node(&s, "prog", NodeKind::Untraced);
        assert_eq!((u.col, u.value_in), (2, 40.0));
        // The programme is as tall as what it received.
        let p = node(&s, "prog", NodeKind::Object);
        assert!((p.h - node(&s, "state", NodeKind::Object).h).abs() < 1e-9);
        // Bands leave the programme stacked without overlap.
        let mut outs: Vec<&SankeyLink> = s
            .links
            .iter()
            .filter(|l| s.nodes[l.from].id == "prog")
            .collect();
        outs.sort_by(|a, b| a.y0.total_cmp(&b.y0));
        for w in outs.windows(2) {
            assert!((w[0].y0 + w[0].w - w[1].y0).abs() < 1e-9);
        }
        assert!(s
            .nodes
            .iter()
            .all(|n| n.y >= 0.0 && n.y + n.h <= 300.0 + 1e-9));
    }

    #[test]
    fn pairs_aggregate_and_keep_their_events() {
        let ts = [
            t("a", "b", 10.0, "e1"),
            t("a", "b", 5.0, "e2"),
            t("a", "a", 99.0, "e3"),
        ];
        let s = layout(&ts, 100.0, 4.0);
        assert_eq!(s.links.len(), 1);
        assert_eq!(s.links[0].value, 15.0);
        assert_eq!(s.links[0].events, vec!["e1".to_string(), "e2".to_string()]);
    }

    #[test]
    fn cycles_are_flagged_not_dropped() {
        // a → b → c → a: round trip.
        let ts = [
            t("a", "b", 10.0, "e1"),
            t("b", "c", 10.0, "e2"),
            t("c", "a", 10.0, "e3"),
        ];
        let s = layout(&ts, 100.0, 4.0);
        assert_eq!(s.links.iter().filter(|l| l.back).count(), 1);
        assert_eq!(s.links.iter().filter(|l| l.events.len() == 1).count(), 3);
        // Columns still increase along forward edges.
        for l in s.links.iter().filter(|l| !l.back) {
            assert!(s.nodes[l.to].col > s.nodes[l.from].col);
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let ts = [
            t("x", "y", 3.0, "1"),
            t("x", "z", 7.0, "2"),
            t("z", "w", 2.0, "3"),
        ];
        assert_eq!(layout(&ts, 200.0, 8.0), layout(&ts, 200.0, 8.0));
        assert_eq!(layout(&[], 200.0, 8.0), Sankey::default());
    }

    #[test]
    fn amounts_format_per_language() {
        assert_eq!(compact_amount(1_234_567.0, true), "1,2 M");
        assert_eq!(compact_amount(1_234_567.0, false), "1.2 M");
        assert_eq!(compact_amount(2_000_000_000.0, true), "2 Md");
        assert_eq!(compact_amount(340_000.0, false), "340 k");
        assert_eq!(compact_amount(12_500.0, false), "12,500");
        assert_eq!(full_amount(1_250.5, true), "1\u{202f}250,50");
        assert_eq!(full_amount(1_000_000.0, false), "1,000,000");
        assert_eq!(full_amount(-3.0, false), "-3");
    }
}
