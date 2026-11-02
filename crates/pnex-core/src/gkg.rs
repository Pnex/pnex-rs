//! Reading the GDELT Global Knowledge Graph (account-mapping.md D539): the
//! press of the world as tab-separated files, one every 15 minutes, with the
//! organizations each article names. Pure: the server fetches the files,
//! this keeps the articles naming a company followed, then one signal a day.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::org_resolve::tokens;

const COL_DATE: usize = 1;
const COL_SOURCE: usize = 3;
const COL_URL: usize = 4;
const COL_ORGANIZATIONS: usize = 13;
const COL_TONE: usize = 15;
const COL_TRANSLATION: usize = 25;
const COL_EXTRAS: usize = 26;

/// A company followed: the names it goes by, and how to find it again.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Watched {
    pub names: Vec<String>,
    /// `(property, value)` of a unique property of the company.
    pub key: (String, String),
}

/// The file name of each slice of the feed from `end` back over `hours`:
/// `YYYYMMDDHHMMSS.gkg.csv.zip` and its translated twin, newest first.
pub fn file_names(end: chrono::DateTime<chrono::Utc>, hours: u32) -> Vec<String> {
    use chrono::{Duration, DurationRound};
    let floor = end.duration_trunc(Duration::minutes(15)).unwrap_or(end);
    (0..hours as i64 * 4)
        .flat_map(|i| {
            let stamp = (floor - Duration::minutes(15 * i)).format("%Y%m%d%H%M%S");
            [
                format!("{stamp}.gkg.csv.zip"),
                format!("{stamp}.translation.gkg.csv.zip"),
            ]
        })
        .collect()
}

/// `&#xE9;`, `&#233;` and the common named entities of a page title.
fn decoded(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = rest
            .find(';')
            .filter(|end| *end <= 10)
            .map(|end| (&rest[1..end], end));
        let found = entity.and_then(|(e, end)| {
            let c = match e {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => e
                    .strip_prefix("#x")
                    .or_else(|| e.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| e.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, end))
        });
        match found {
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

fn title_of(extras: &str) -> Option<String> {
    let start = extras.find("<PAGE_TITLE>")? + "<PAGE_TITLE>".len();
    let len = extras[start..].find("</PAGE_TITLE>")?;
    let title = decoded(&extras[start..start + len]);
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

/// `20261102121500` → `2026-11-02`.
fn day_of(stamp: &str) -> Option<String> {
    let (y, m, d) = (stamp.get(0..4)?, stamp.get(4..6)?, stamp.get(6..8)?);
    Some(format!("{y}-{m}-{d}"))
}

/// Finds the companies an article names. Whole words from the start of the
/// organization: `acme gaz france` names ACME GAZ, `mercurey` does
/// not name MERCURE.
pub struct Matcher {
    names: Vec<(Vec<String>, usize)>,
    watched: Vec<Watched>,
}

impl Matcher {
    pub fn new(watched: Vec<Watched>) -> Self {
        let mut names = Vec::new();
        for (i, w) in watched.iter().enumerate() {
            for n in &w.names {
                let t = tokens(n);
                // One or two letters name nothing reliably.
                if !t.is_empty() && t.iter().map(String::len).sum::<usize>() >= 3 {
                    names.push((t, i));
                }
            }
        }
        Self { names, watched }
    }

    /// Whether the headline itself names this company, whole words.
    fn in_title(&self, i: usize, title: &[String]) -> bool {
        self.names
            .iter()
            .filter(|(_, w)| *w == i)
            .any(|(name, _)| title.windows(name.len()).any(|w| w == name.as_slice()))
    }

    fn named_in(&self, organizations: &str) -> Vec<usize> {
        let mut out = Vec::new();
        let orgs: HashSet<&str> = organizations
            .split(';')
            .filter_map(|o| o.split(',').next())
            .filter(|o| !o.is_empty())
            .collect();
        for org in orgs {
            let t = tokens(org);
            for (name, i) in &self.names {
                if t.starts_with(name) && !out.contains(i) {
                    out.push(*i);
                }
            }
        }
        out
    }

    /// The records of one file, as the mapping reads them.
    pub fn records(&self, tsv: &str) -> Vec<Value> {
        let mut out = Vec::new();
        for line in tsv.lines() {
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() <= COL_EXTRAS || c[COL_ORGANIZATIONS].is_empty() {
                continue;
            }
            let (hits, url) = (self.named_in(c[COL_ORGANIZATIONS]), c[COL_URL]);
            if hits.is_empty() || url.is_empty() {
                continue;
            }
            let Some(title) = title_of(c[COL_EXTRAS]) else {
                continue;
            };
            let lang = c[COL_TRANSLATION]
                .split("srclc:")
                .nth(1)
                .and_then(|s| s.split(';').next())
                .unwrap_or("eng");
            let tone = c[COL_TONE]
                .split(',')
                .next()
                .and_then(|t| t.parse::<f64>().ok())
                .unwrap_or_default();
            let words = tokens(&title);
            for i in hits {
                let (key, value) = &self.watched[i].key;
                out.push(json!({
                    "title": title.chars().take(255).collect::<String>(),
                    "summary": title.chars().take(1000).collect::<String>(),
                    "url": url,
                    "source": c[COL_SOURCE],
                    "date": day_of(c[COL_DATE]).unwrap_or_default(),
                    "lang": lang,
                    "tone": tone,
                    "in_title": self.in_title(i, &words),
                    "via": key,
                    "account": value,
                }));
            }
        }
        out
    }
}

/// One signal per company per day, whatever the coverage: a reviewer judges
/// news, not articles. Measured on real files, the headlines of one event
/// barely share a word ("plunges", "crashes", "shares fall 26%"), so nothing
/// finer groups them. Headlines naming the company only: a passing mention
/// is market roundups, or another organization of that name. The headline
/// closest to the others leads, the others are in the summary, where a second
/// event of the day stays readable.
pub fn digest(records: Vec<Value>) -> Vec<Value> {
    let text = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
    let mut by_company: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for r in records {
        let company = format!("{}:{}", text(&r, "via"), text(&r, "account"));
        let headline = tokens(&text(&r, "title")).join(" ");
        if r["in_title"].as_bool() == Some(true) && seen.insert((company.clone(), headline)) {
            by_company.entry(company).or_default().push(r);
        }
    }
    by_company
        .into_iter()
        .filter_map(|(company, mut group)| {
            // Oldest first: the earliest of equals leads, on every run.
            group.sort_by_key(|r| text(r, "date"));
            let words: Vec<HashSet<String>> = group
                .iter()
                .map(|r| {
                    let mut t = tokens(&text(r, "title"));
                    t.retain(|w| w.len() >= 4);
                    t.into_iter().collect()
                })
                .collect();
            let shared = |i: usize| -> usize {
                let all: usize = words
                    .iter()
                    .map(|other| words[i].intersection(other).count())
                    .sum();
                // Its own words would favour the longest headline.
                all - words[i].len()
            };
            // Among the headlines the reviewer can read, when there are any.
            // shortcut: French and English, the org's language if ever set.
            let readable = |i: usize| matches!(group[i]["lang"].as_str(), Some("fra" | "eng"));
            let lead = (0..group.len())
                .rev()
                .max_by_key(|i| (readable(*i), shared(*i)))?;
            let mut r = group[lead].clone();
            let others: Vec<String> = group
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != lead)
                .map(|(_, o)| text(o, "title"))
                .collect();
            let summary = std::iter::once(text(&r, "title"))
                .chain(others)
                .collect::<Vec<_>>()
                .join(" · ");
            r["summary"] = json!(summary.chars().take(1000).collect::<String>());
            r["ref"] = json!(format!("press:{company}:{}", text(&r, "date")));
            Some(r)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(date: &str, url: &str, orgs: &str, translation: &str, title: &str) -> String {
        let mut c = vec![""; 27];
        let extras = format!("<PAGE_AUTHORS>x</PAGE_AUTHORS><PAGE_TITLE>{title}</PAGE_TITLE>");
        c[COL_DATE] = date;
        c[COL_SOURCE] = "news.example.org";
        c[COL_URL] = url;
        c[COL_ORGANIZATIONS] = orgs;
        c[COL_TONE] = "-2.5,1,2";
        c[COL_TRANSLATION] = translation;
        c[COL_EXTRAS] = &extras;
        c.join("\t")
    }

    fn matcher() -> Matcher {
        Matcher::new(vec![
            Watched {
                names: vec!["GLOBEX".into()],
                key: ("siren".into(), "732829320".into()),
            },
            Watched {
                names: vec!["VOLTA DE FRANCE".into(), "VDF".into()],
                key: ("lei".into(), "LEI1".into()),
            },
            Watched {
                names: vec!["X".into()],
                key: ("siren".into(), "1".into()),
            },
        ])
    }

    #[test]
    fn an_article_gives_one_record_per_company_it_names() {
        let tsv = line(
            "20261102121500",
            "https://news.example.org/a",
            "globex aircraft engines,10;globex,50;vdf,90;hooli,120",
            "srclc:fra;eng:Moses",
            "Globex &#xE9;tend son usine &amp; recrute",
        );
        let r = matcher().records(&tsv);
        assert_eq!(r.len(), 2, "{r:?}");
        assert_eq!(r[0]["title"], "Globex étend son usine & recrute");
        assert_eq!(r[0]["date"], "2026-11-02");
        assert_eq!(r[0]["lang"], "fra");
        assert_eq!(r[0]["tone"], -2.5);
        let keys: Vec<(&str, &str)> = r
            .iter()
            .map(|x| (x["via"].as_str().unwrap(), x["account"].as_str().unwrap()))
            .collect();
        assert!(keys.contains(&("siren", "732829320")) && keys.contains(&("lei", "LEI1")));
    }

    #[test]
    fn a_word_that_names_no_one_does_not_hide_a_company() {
        let tsv = line("20261102121500", "https://a", "groupe globex,1", "", "t");
        assert_eq!(matcher().records(&tsv).len(), 1);
    }

    #[test]
    fn a_name_is_matched_on_whole_words_from_the_start() {
        let m = matcher();
        for orgs in [
            "globexe,1",
            "musee du globex,1",
            "x corp,1",
            "compagnie générale,1",
        ] {
            let tsv = line("20261102121500", "https://a", orgs, "", "t");
            assert!(m.records(&tsv).is_empty(), "{orgs}");
        }
    }

    #[test]
    fn a_line_without_url_title_or_columns_is_left_out() {
        let m = matcher();
        assert!(m.records("a\tb\tc").is_empty());
        let no_url = line("20261102121500", "", "globex,1", "", "t");
        assert!(m.records(&no_url).is_empty());
        let mut c: Vec<&str> = vec![""; 27];
        c[COL_URL] = "https://a";
        c[COL_ORGANIZATIONS] = "globex,1";
        c[COL_EXTRAS] = "<PAGE_AUTHORS>x</PAGE_AUTHORS>";
        assert!(m.records(&c.join("\t")).is_empty(), "no title");
    }

    /// Real headlines of one event (GDELT, 2026-11-02), and market noise.
    #[test]
    fn a_days_coverage_is_one_signal_led_by_the_central_headline() {
        let m = Matcher::new(vec![Watched {
            names: vec!["INITECH".into()],
            key: ("siren".into(), "1".into()),
        }]);
        let tsv = [
            (
                "https://a",
                "Initech plunges as it slashes outlook second time in 2026",
            ),
            (
                "https://b",
                "Initech shares fall 26% in early Paris trading after forecast cut",
            ),
            (
                "https://c",
                "French Rail Giant Initech Crashes On Painful Second Warning",
            ),
            (
                "https://d",
                "Luas maker Initech shares fall 28% after train maker cuts profit forecast",
            ),
            (
                "https://e",
                "European Markets Rise As Oil Falls On Iran Peace Hopes",
            ),
            (
                "https://f",
                "Initech plunges as it slashes outlook second time in 2026!",
            ),
        ]
        .map(|(url, title)| line("20261102120000", url, "initech,1", "", title))
        .join("\n");
        let r = digest(m.records(&tsv));
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(r[0]["url"], "https://b", "{r:?}");
        assert_eq!(r[0]["ref"], "press:siren:1:2026-11-02");
        let summary = r[0]["summary"].as_str().unwrap();
        assert!(
            summary.contains("Crashes") && summary.contains("Luas"),
            "{summary}"
        );
        assert!(!summary.contains("Iran"), "{summary}");
        assert_eq!(summary.matches("plunges").count(), 1, "{summary}");
    }

    #[test]
    fn a_readable_headline_leads_over_a_more_repeated_one() {
        let m = Matcher::new(vec![Watched {
            names: vec!["UMBRELLA".into()],
            key: ("siren".into(), "1".into()),
        }]);
        let tsv = [
            (
                "https://a",
                "srclc:sqi;eng:Moses",
                "ZYRTARE: Umbrella Clio vetura me e shitur",
            ),
            (
                "https://b",
                "srclc:sqi;eng:Moses",
                "Umbrella Clio vetura me e shitur ne Kosove",
            ),
            ("https://c", "", "Umbrella to build a new plant"),
        ]
        .map(|(url, translation, title)| {
            line("20261102120000", url, "umbrella,1", translation, title)
        })
        .join("\n");
        assert_eq!(digest(m.records(&tsv))[0]["url"], "https://c");
    }

    #[test]
    fn a_passing_mention_is_no_signal_and_companies_stay_apart() {
        let m = Matcher::new(vec![
            Watched {
                names: vec!["MERCURE".into()],
                key: ("siren".into(), "1".into()),
            },
            Watched {
                names: vec!["GLOBEX".into()],
                key: ("siren".into(), "2".into()),
            },
        ]);
        let tsv = [
            line(
                "20261102130000",
                "https://n",
                "mercure democratic movement,1",
                "",
                "Sakaja dumps UDA for ODM",
            ),
            line(
                "20261102120000",
                "https://o",
                "mercure,1",
                "",
                "Mercure supprime 650 postes",
            ),
            line(
                "20261102120000",
                "https://s",
                "globex,1",
                "",
                "Globex recrute",
            ),
        ]
        .join("\n");
        let r = digest(m.records(&tsv));
        let urls: Vec<&str> = r.iter().map(|x| x["url"].as_str().unwrap()).collect();
        assert_eq!(urls, ["https://o", "https://s"], "{r:?}");
    }

    #[test]
    fn the_files_cover_the_window_from_the_last_quarter() {
        let end = "2026-11-02T12:07:30Z".parse().unwrap();
        let f = file_names(end, 1);
        assert_eq!(f.len(), 8);
        assert_eq!(f[0], "20261102120000.gkg.csv.zip");
        assert_eq!(f[1], "20261102120000.translation.gkg.csv.zip");
        assert_eq!(f[7], "20261102111500.translation.gkg.csv.zip");
    }
}
