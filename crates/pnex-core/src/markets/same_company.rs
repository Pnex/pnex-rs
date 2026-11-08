//! A company created from its SEC number found again by its LEI
//! (markets-research.md D812): the SEC does not publish LEIs, so the SEC
//! name is looked up in GLEIF and only a single candidate with the same
//! legal name is kept. The result is always a proposal, never a merge.
//! Pure code: no I/O.

use serde_json::Value;

/// One GLEIF record: its LEI and the names it is known by.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub lei: String,
    pub legal_name: String,
    pub other_names: Vec<String>,
}

/// A legal name reduced to compare a register's spelling with another's:
/// `NVIDIA CORP` and `NVIDIA Corporation`, `COCA COLA CO` and `The Coca-Cola
/// Company` give the same key. Accents, punctuation and a leading `THE` go;
/// a legal form keeps its abbreviation.
pub fn legal_key(name: &str) -> String {
    let folded: String = crate::fold::fold(name)
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                ' '
            }
        })
        .collect();
    let mut words: Vec<&str> = folded
        .split_whitespace()
        .map(|w| match w {
            "CORPORATION" => "CORP",
            "INCORPORATED" => "INC",
            "COMPANY" => "CO",
            "LIMITED" => "LTD",
            other => other,
        })
        .collect();
    if words.len() > 1 && words[0] == "THE" {
        words.remove(0);
    }
    words.join(" ")
}

/// The records of a GLEIF `lei-records` answer (JSON:API).
pub fn candidates(doc: &Value) -> Option<Vec<Candidate>> {
    let mut out = Vec::new();
    for r in doc.get("data")?.as_array()? {
        let a = r.get("attributes")?;
        let lei = a.get("lei")?.as_str()?;
        if lei.len() != 20 || !lei.bytes().all(|b| b.is_ascii_alphanumeric()) {
            continue;
        }
        let entity = a.get("entity")?;
        let name = |v: &Value| v.get("name").and_then(Value::as_str).map(str::to_string);
        let Some(legal_name) = entity.get("legalName").and_then(name) else {
            continue;
        };
        let other_names = entity
            .get("otherNames")
            .and_then(Value::as_array)
            .map(|v| v.iter().filter_map(name).collect())
            .unwrap_or_default();
        out.push(Candidate {
            lei: lei.to_ascii_uppercase(),
            legal_name,
            other_names,
        });
    }
    Some(out)
}

/// The LEI of `name` when exactly one candidate has the same legal key
/// (its legal name or one of its other names); `None` when none or several.
pub fn unique_match<'a>(name: &str, found: &'a [Candidate]) -> Option<&'a Candidate> {
    let key = legal_key(name);
    if key.is_empty() {
        return None;
    }
    let mut hits = found.iter().filter(|c| {
        std::iter::once(&c.legal_name)
            .chain(&c.other_names)
            .any(|n| legal_key(n) == key)
    });
    let first = hits.next()?;
    hits.all(|c| c.lei == first.lei).then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(lei: &str, legal: &str, others: &[&str]) -> Value {
        json!({"attributes": {"lei": lei, "entity": {
            "legalName": {"name": legal},
            "otherNames": others.iter().map(|n| json!({"name": n})).collect::<Vec<_>>()
        }}})
    }

    #[test]
    fn register_spellings_meet() {
        assert_eq!(legal_key("NVIDIA CORP"), legal_key("NVIDIA Corporation"));
        assert_eq!(
            legal_key("COCA COLA CO"),
            legal_key("The Coca-Cola Company")
        );
        assert_eq!(legal_key("MERCK & CO INC"), legal_key("Merck & Co., Inc."));
        assert_eq!(legal_key("Société Générale"), "SOCIETE GENERALE");
        assert_ne!(legal_key("ACME INC"), legal_key("ACME HOLDINGS INC"));
    }

    #[test]
    fn only_a_single_candidate_is_kept() {
        let doc = json!({"data": [
            record("5493000000000000ACME", "Acme Corporation", &[]),
            record("5493000000000000AHLD", "Acme Holdings Inc.", &[]),
            record("bad", "Acme Corp", &[]),
        ]});
        let found = candidates(&doc).unwrap();
        assert_eq!(found.len(), 2, "a malformed LEI is left aside");
        assert_eq!(
            unique_match("ACME CORP", &found).unwrap().lei,
            "5493000000000000ACME"
        );
        assert!(
            unique_match("ACME", &found).is_none(),
            "no legal name is just ACME"
        );

        let two = json!({"data": [
            record("5493000000000000AAAA", "Acme Corporation", &[]),
            record("5493000000000000BBBB", "Old Name", &["ACME CORP"]),
        ]});
        assert!(
            unique_match("ACME CORP", &candidates(&two).unwrap()).is_none(),
            "two companies: ambiguous"
        );
        assert!(candidates(&json!({"errors": []})).is_none());
    }
}
