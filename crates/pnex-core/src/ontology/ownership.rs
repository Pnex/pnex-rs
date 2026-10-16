//! Effective ownership along chains of `owns` links (cases-money.md K56):
//! the share of an owner in a company is the product of the shares along
//! each chain, summed over the chains. A computed beneficial owner is a
//! deduction, never a fact: it always comes with the chains behind it.

use std::collections::BTreeMap;

/// Share above which an owner is a computed beneficial owner, in percent
/// (EU anti-money-laundering threshold).
pub const BENEFICIAL_OWNER_PERCENT: f64 = 25.0;
/// Longest chain followed; cross-holdings loop, simple paths only.
const MAX_DEPTH: usize = 8;

/// One `owns` link: `share` in percent, `None` when not documented.
#[derive(Debug, Clone, PartialEq)]
pub struct Holding {
    pub owner: String,
    pub owned: String,
    pub share: Option<f64>,
}

/// An owner's effective stake in a company (or, seen downstream, a
/// company's in which the owner has a stake).
#[derive(Debug, Clone, PartialEq)]
pub struct Stake {
    pub id: String,
    /// Sum over the chains of the product of the documented shares.
    pub percent: f64,
    /// A chain has a link without a share: the real stake may be higher.
    pub partial: bool,
    /// Shortest chain, in links (1 = direct).
    pub depth: usize,
    pub chains: usize,
}

/// Every direct or indirect owner of `target`, largest stake first.
pub fn owners_of(holdings: &[Holding], target: &str) -> Vec<Stake> {
    walk(holdings, target, true)
}

/// Every company `owner` holds directly or through others.
pub fn held_by(holdings: &[Holding], owner: &str) -> Vec<Stake> {
    walk(holdings, owner, false)
}

fn walk(holdings: &[Holding], start: &str, upstream: bool) -> Vec<Stake> {
    let mut out: BTreeMap<String, Stake> = BTreeMap::new();
    let mut path = vec![start.to_string()];
    step(holdings, upstream, &mut path, 100.0, false, &mut out);
    let mut v: Vec<Stake> = out.into_values().collect();
    v.sort_by(|a, b| b.percent.total_cmp(&a.percent).then(a.id.cmp(&b.id)));
    v
}

fn step(
    holdings: &[Holding],
    upstream: bool,
    path: &mut Vec<String>,
    percent: f64,
    partial: bool,
    out: &mut BTreeMap<String, Stake>,
) {
    if path.len() > MAX_DEPTH {
        return;
    }
    let here = path.last().cloned().unwrap_or_default();
    for h in holdings {
        let (near, far) = if upstream {
            (&h.owned, &h.owner)
        } else {
            (&h.owner, &h.owned)
        };
        if *near != here || path.contains(far) {
            continue;
        }
        let share = h.share.map(|s| s.clamp(0.0, 100.0));
        let p = percent * share.unwrap_or(0.0) / 100.0;
        let part = partial || share.is_none();
        let depth = path.len();
        let e = out.entry(far.clone()).or_insert_with(|| Stake {
            id: far.clone(),
            percent: 0.0,
            partial: false,
            depth,
            chains: 0,
        });
        e.percent += p;
        e.partial |= part;
        e.depth = e.depth.min(depth);
        e.chains += 1;
        path.push(far.clone());
        step(holdings, upstream, path, p, part, out);
        path.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(owner: &str, owned: &str, share: Option<f64>) -> Holding {
        Holding {
            owner: owner.into(),
            owned: owned.into(),
            share,
        }
    }

    #[test]
    fn chains_multiply_and_add_up() {
        // Ann holds 60 % of Hold, which holds 50 % of Op; Ann also holds
        // 10 % of Op directly: 0.6 × 0.5 + 0.1 = 40 %.
        let hs = [
            h("ann", "hold", Some(60.0)),
            h("hold", "op", Some(50.0)),
            h("ann", "op", Some(10.0)),
        ];
        let o = owners_of(&hs, "op");
        assert_eq!(o[0].id, "hold");
        let ann = o.iter().find(|s| s.id == "ann").unwrap();
        assert!((ann.percent - 40.0).abs() < 1e-9);
        assert_eq!((ann.depth, ann.chains, ann.partial), (1, 2, false));
        assert!(ann.percent >= BENEFICIAL_OWNER_PERCENT);
        let held = held_by(&hs, "ann");
        assert_eq!(
            held.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["hold", "op"]
        );
    }

    #[test]
    fn missing_share_marks_partial_and_cycles_end() {
        let hs = [
            h("bob", "x", None),
            h("x", "y", Some(100.0)),
            h("y", "x", Some(30.0)),
        ];
        let o = owners_of(&hs, "y");
        let bob = o.iter().find(|s| s.id == "bob").unwrap();
        assert!(bob.partial);
        assert_eq!(bob.percent, 0.0);
        // The cross-holding does not loop forever.
        assert!(o.iter().all(|s| s.chains < 10));
    }
}
