//! Subsidiaries a US-listed group declares in the Exhibit 21 of its annual
//! report (10-K), worldwide (test-grandeur-nature-2026-11-06.md F6). Pure:
//! the server fetches the filing index and the exhibit. The exhibit has no
//! fixed form: a table (name, jurisdiction) or lines of text, so a row this
//! reader does not understand is left out, never guessed.

/// Most subsidiaries kept per exhibit.
pub const SUBSIDIARIES_MAX: usize = 5000;

/// One declared subsidiary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    pub name: String,
    /// The jurisdiction as written (`Delaware`, `England and Wales`…).
    pub jurisdiction: String,
    /// ISO 3166 alpha-2 when the jurisdiction is known; a US state is `US`.
    pub country: Option<&'static str>,
}

impl Declared {
    /// The key a subsidiary is found again by: country (or jurisdiction)
    /// and name, upper-cased with spaces and punctuation folded.
    pub fn key(&self) -> String {
        let place = self
            .country
            .map(str::to_string)
            .unwrap_or_else(|| fold(&self.jurisdiction));
        format!("{place}|{}", fold(&self.name))
    }
}

fn fold(s: &str) -> String {
    // Accents go through the shared fold first: "Société" and "Societe" meet.
    crate::fold::fold(s)
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The address of the EX-21 document in a filing index page
/// (`<accession>-index.htm`): the row whose type is `EX-21` or `EX-21.x`.
pub fn exhibit_href(index_html: &str) -> Option<String> {
    for row in rows(index_html) {
        let is21 = row.iter().any(|c| {
            let t = text_of(c).to_ascii_uppercase();
            t == "EX-21" || t.starts_with("EX-21.")
        });
        if !is21 {
            continue;
        }
        for cell in &row {
            if let Some(h) = href_of(cell) {
                return Some(h);
            }
        }
    }
    None
}

/// The subsidiaries of an exhibit, HTML or plain text.
pub fn subsidiaries(doc: &str) -> Vec<Declared> {
    let mut out: Vec<Declared> = if doc.to_ascii_lowercase().contains("<tr") {
        rows(doc)
            .into_iter()
            .filter_map(|row| {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| text_of(c))
                    .filter(|t| !t.is_empty())
                    .collect();
                match cells.as_slice() {
                    [name, .., place] if cells.len() >= 2 => declared(name, place),
                    // One cell: "Name (Jurisdiction)".
                    [one] => from_line(one),
                    _ => None,
                }
            })
            .collect()
    } else {
        doc.lines().filter_map(from_line).collect()
    };
    let mut seen = std::collections::HashSet::new();
    out.retain(|d| seen.insert(d.key()));
    out.truncate(SUBSIDIARIES_MAX);
    out
}

/// A line of text: `Name (Jurisdiction)`, or name and jurisdiction apart by
/// a tab, two spaces or a run of dots.
fn from_line(line: &str) -> Option<Declared> {
    let line = line.trim();
    if let (Some(open), true) = (line.rfind('('), line.ends_with(')')) {
        return declared(&line[..open], &line[open + 1..line.len() - 1]);
    }
    let cut = line
        .find('\t')
        .or_else(|| line.find("  "))
        .or_else(|| line.find(".."))?;
    let name = &line[..cut];
    let place = line[cut..].trim_matches(|c: char| c == '.' || c.is_whitespace());
    declared(name, place)
}

fn declared(name: &str, place: &str) -> Option<Declared> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let jurisdiction = place.split_whitespace().collect::<Vec<_>>().join(" ");
    let country = country_of(&jurisdiction);
    // A header ("Name of subsidiary | Jurisdiction"), a note or a page
    // number is no subsidiary: the jurisdiction must be one we know, and a
    // name has letters.
    if country.is_none()
        || name.len() < 2
        || name.len() > 255
        || !name.chars().any(char::is_alphabetic)
        || name.to_ascii_lowercase().starts_with("name")
    {
        return None;
    }
    Some(Declared {
        name,
        jurisdiction,
        country,
    })
}

const US_STATES: [&str; 52] = [
    "alabama",
    "alaska",
    "arizona",
    "arkansas",
    "california",
    "colorado",
    "connecticut",
    "delaware",
    "florida",
    "georgia",
    "hawaii",
    "idaho",
    "illinois",
    "indiana",
    "iowa",
    "kansas",
    "kentucky",
    "louisiana",
    "maine",
    "maryland",
    "massachusetts",
    "michigan",
    "minnesota",
    "mississippi",
    "missouri",
    "montana",
    "nebraska",
    "nevada",
    "new hampshire",
    "new jersey",
    "new mexico",
    "new york",
    "north carolina",
    "north dakota",
    "ohio",
    "oklahoma",
    "oregon",
    "pennsylvania",
    "rhode island",
    "south carolina",
    "south dakota",
    "tennessee",
    "texas",
    "utah",
    "vermont",
    "virginia",
    "washington",
    "west virginia",
    "wisconsin",
    "wyoming",
    "district of columbia",
    "puerto rico",
];

/// Jurisdictions as exhibits write them → ISO alpha-2.
// shortcut: the usual names; an unknown jurisdiction leaves its row out. Add
// a row when a real exhibit shows one.
const PLACES: [(&str, &str); 120] = [
    ("united states", "US"),
    ("usa", "US"),
    ("u.s.", "US"),
    ("united kingdom", "GB"),
    ("england", "GB"),
    ("england and wales", "GB"),
    ("scotland", "GB"),
    ("wales", "GB"),
    ("northern ireland", "GB"),
    ("uk", "GB"),
    ("ireland", "IE"),
    ("france", "FR"),
    ("germany", "DE"),
    ("netherlands", "NL"),
    ("the netherlands", "NL"),
    ("belgium", "BE"),
    ("luxembourg", "LU"),
    ("switzerland", "CH"),
    ("austria", "AT"),
    ("italy", "IT"),
    ("spain", "ES"),
    ("portugal", "PT"),
    ("sweden", "SE"),
    ("norway", "NO"),
    ("denmark", "DK"),
    ("finland", "FI"),
    ("iceland", "IS"),
    ("poland", "PL"),
    ("czech republic", "CZ"),
    ("czechia", "CZ"),
    ("slovakia", "SK"),
    ("hungary", "HU"),
    ("romania", "RO"),
    ("bulgaria", "BG"),
    ("greece", "GR"),
    ("cyprus", "CY"),
    ("malta", "MT"),
    ("croatia", "HR"),
    ("slovenia", "SI"),
    ("serbia", "RS"),
    ("estonia", "EE"),
    ("latvia", "LV"),
    ("lithuania", "LT"),
    ("ukraine", "UA"),
    ("russia", "RU"),
    ("russian federation", "RU"),
    ("turkey", "TR"),
    ("türkiye", "TR"),
    ("israel", "IL"),
    ("united arab emirates", "AE"),
    ("uae", "AE"),
    ("saudi arabia", "SA"),
    ("qatar", "QA"),
    ("kuwait", "KW"),
    ("bahrain", "BH"),
    ("oman", "OM"),
    ("egypt", "EG"),
    ("morocco", "MA"),
    ("south africa", "ZA"),
    ("nigeria", "NG"),
    ("kenya", "KE"),
    ("ghana", "GH"),
    ("canada", "CA"),
    ("mexico", "MX"),
    ("brazil", "BR"),
    ("argentina", "AR"),
    ("chile", "CL"),
    ("colombia", "CO"),
    ("peru", "PE"),
    ("uruguay", "UY"),
    ("costa rica", "CR"),
    ("panama", "PA"),
    ("bermuda", "BM"),
    ("cayman islands", "KY"),
    ("british virgin islands", "VG"),
    ("bahamas", "BS"),
    ("barbados", "BB"),
    ("jersey", "JE"),
    ("guernsey", "GG"),
    ("isle of man", "IM"),
    ("gibraltar", "GI"),
    ("china", "CN"),
    ("people's republic of china", "CN"),
    ("prc", "CN"),
    ("hong kong", "HK"),
    ("macau", "MO"),
    ("taiwan", "TW"),
    ("japan", "JP"),
    ("korea", "KR"),
    ("south korea", "KR"),
    ("republic of korea", "KR"),
    ("india", "IN"),
    ("singapore", "SG"),
    ("malaysia", "MY"),
    ("thailand", "TH"),
    ("indonesia", "ID"),
    ("philippines", "PH"),
    ("vietnam", "VN"),
    ("viet nam", "VN"),
    ("pakistan", "PK"),
    ("bangladesh", "BD"),
    ("sri lanka", "LK"),
    ("australia", "AU"),
    ("new zealand", "NZ"),
    ("mauritius", "MU"),
    ("liechtenstein", "LI"),
    ("monaco", "MC"),
    ("kazakhstan", "KZ"),
    ("jordan", "JO"),
    ("lebanon", "LB"),
    ("tunisia", "TN"),
    ("algeria", "DZ"),
    ("ecuador", "EC"),
    ("venezuela", "VE"),
    ("guatemala", "GT"),
    ("dominican republic", "DO"),
    ("curacao", "CW"),
    ("curaçao", "CW"),
    ("aruba", "AW"),
    ("labuan", "MY"),
];

/// The ISO code of a jurisdiction as an exhibit writes it.
pub fn country_of(jurisdiction: &str) -> Option<&'static str> {
    let j = jurisdiction
        .trim()
        .trim_end_matches('.')
        .to_lowercase()
        .replace(['\u{a0}', ','], " ");
    let j = j.split_whitespace().collect::<Vec<_>>().join(" ");
    if US_STATES.contains(&j.as_str()) {
        return Some("US");
    }
    PLACES.iter().find(|(n, _)| *n == j).map(|(_, iso)| *iso)
}

/// The rows of every table of a page, each row its raw cells.
fn rows(html: &str) -> Vec<Vec<String>> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(start) = lower[at..].find("<tr").map(|i| at + i) {
        let end = lower[start..]
            .find("</tr")
            .map(|i| start + i)
            .unwrap_or(lower.len());
        let row = &html[start..end];
        let row_lower = &lower[start..end];
        let mut cells = Vec::new();
        let mut c = 0;
        while let Some(open) = row_lower[c..].find("<td").map(|i| c + i) {
            let body = row_lower[open..]
                .find('>')
                .map(|i| open + i + 1)
                .unwrap_or(row.len());
            let close = row_lower[body..]
                .find("</td")
                .map(|i| body + i)
                .unwrap_or(row.len());
            cells.push(row[body..close].to_string());
            c = close;
        }
        out.push(cells);
        at = end.max(start + 3);
    }
    out
}

/// The visible text of a cell: tags dropped, entities read, spaces folded.
fn text_of(cell: &str) -> String {
    let mut out = String::with_capacity(cell.len());
    let mut in_tag = false;
    for ch in cell.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let out = entities(&out).replace('\u{a0}', " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `&amp;`, `&nbsp;`, `&rsquo;` and every numeric entity (`&#47;`, `&#x2F;`).
fn entities(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest.find(';').filter(|end| *end <= 10).and_then(|end| {
            let name = &rest[1..end];
            let c = match name {
                "amp" => Some('&'),
                "nbsp" => Some(' '),
                "rsquo" | "lsquo" => Some('\''),
                "quot" => Some('"'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                _ => {
                    let n = name.strip_prefix('#')?;
                    let code = match n.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => n.parse().ok()?,
                    };
                    char::from_u32(code)
                }
            }?;
            Some((c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn href_of(cell: &str) -> Option<String> {
    let lower = cell.to_ascii_lowercase();
    let at = lower.find("href=")? + 5;
    let rest = &cell[at..];
    let quote = rest.chars().next()?;
    let (start, end) = if quote == '"' || quote == '\'' {
        (1, rest[1..].find(quote)? + 1)
    } else {
        (0, rest.find(|c: char| c.is_whitespace() || c == '>')?)
    };
    Some(rest[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_gives_the_exhibit() {
        let index = r#"<table class="tableFile"><tr><th>Seq</th><th>Description</th><th>Document</th><th>Type</th></tr>
            <tr><td>1</td><td>10-K</td><td><a href="/Archives/edgar/data/42/000004226000001/acme-10k.htm">acme-10k.htm</a></td><td>10-K</td></tr>
            <tr><td>5</td><td>SUBSIDIARIES</td><td><a href="/Archives/edgar/data/42/000004226000001/ex21.htm">ex21.htm</a></td><td>EX-21.1</td></tr>
        </table>"#;
        assert_eq!(
            exhibit_href(index).as_deref(),
            Some("/Archives/edgar/data/42/000004226000001/ex21.htm")
        );
        assert_eq!(exhibit_href("<table><tr><td>EX-31</td></tr></table>"), None);
    }

    #[test]
    fn a_table_of_subsidiaries_is_read() {
        let doc = r#"<p>SUBSIDIARIES OF ACME HOLDINGS</p><table>
            <tr><td><b>Name of Subsidiary</b></td><td><b>Jurisdiction of Incorporation</b></td></tr>
            <tr><td>Acme Widgets&nbsp;LLC</td><td>&#160;</td><td>Delaware</td></tr>
            <tr><td>Acme Widgets B.V.</td><td>The Netherlands</td></tr>
            <tr><td>Acme Widgets (UK) Limited</td><td>England and Wales</td></tr>
            <tr><td>Acme Widgets A&#47;S &amp; Co</td><td>Denmark</td></tr>
            <tr><td>Acme Trading Co.</td><td>Atlantis</td></tr>
            <tr><td>Acme Widgets B.V.</td><td>Netherlands</td></tr>
            <tr><td>2</td><td></td></tr>
        </table>"#;
        let got = subsidiaries(doc);
        assert_eq!(got.len(), 4, "{got:?}");
        assert_eq!(got[3].name, "Acme Widgets A/S & Co");
        assert_eq!(got[0].name, "Acme Widgets LLC");
        assert_eq!(got[0].country, Some("US"));
        assert_eq!(got[1].country, Some("NL"));
        assert_eq!(got[2].country, Some("GB"));
        assert_eq!(got[1].key(), "NL|ACME WIDGETS B V");
    }

    #[test]
    fn lines_of_text_are_read() {
        let doc = "EXHIBIT 21\nSubsidiaries of Acme Holdings\n\nAcme Widgets LLC (Delaware)\nAcme Widgets GmbH\tGermany\nAcme Widgets K.K. ........ Japan\nAcme Widgets Pte. Ltd.  Singapore\nPage 1\n";
        let got = subsidiaries(doc);
        let names: Vec<_> = got
            .iter()
            .map(|d| (d.name.as_str(), d.country.unwrap()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Acme Widgets LLC", "US"),
                ("Acme Widgets GmbH", "DE"),
                ("Acme Widgets K.K.", "JP"),
                ("Acme Widgets Pte. Ltd.", "SG")
            ]
        );
    }

    #[test]
    fn key_folds_accents() {
        assert_eq!(fold("Société Générale, S.A."), fold("SOCIETE GENERALE S A"));
    }

    #[test]
    fn jurisdictions_fold_to_iso() {
        assert_eq!(country_of("New York"), Some("US"));
        assert_eq!(country_of("Cayman Islands."), Some("KY"));
        assert_eq!(country_of("People's Republic of China"), Some("CN"));
        assert_eq!(country_of("Narnia"), None);
    }
}
