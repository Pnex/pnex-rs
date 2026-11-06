//! Reading the GDELT 2.0 Events exports (markets-research.md D740): one
//! tab-separated file every 15 minutes, one coded event per line. The raw
//! feed is too noisy to keep line by line, so the lines of the watched
//! countries and codes become one lead per (country, code family, day),
//! always unconfirmed. Pure: the server fetches the files.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{json, Value};

/// What a run keeps: countries (ISO 3166 alpha-2, converted from the feed's
/// FIPS 10-4), CAMEO code families, and the per-line thresholds.
#[derive(Debug, Clone, PartialEq)]
pub struct EventsFilter {
    pub countries: BTreeSet<String>,
    /// Code prefixes, `18`, `19`, `163`…; the longest matching one wins.
    pub codes: Vec<String>,
    pub min_sources: u32,
    pub max_goldstein: f64,
}

/// The file name of each slice from `end` back over `hours`, newest first.
pub fn file_names(end: chrono::DateTime<chrono::Utc>, hours: u32) -> Vec<String> {
    use chrono::{Duration, DurationRound};
    let floor = end.duration_trunc(Duration::minutes(15)).unwrap_or(end);
    (0..hours as i64 * 4)
        .map(|i| {
            let stamp = (floor - Duration::minutes(15 * i)).format("%Y%m%d%H%M%S");
            format!("{stamp}.export.CSV.zip")
        })
        .collect()
}

/// GDELT writes countries in FIPS 10-4; PNEX keeps ISO 3166 alpha-2. Both
/// halves of Palestine (GZ, WE) are PS. Codes of no country (oceans, old
/// states) map to nothing and their lines are left out.
const FIPS_TO_ISO: [(&str, &str); 247] = [
    ("AA", "AW"),
    ("AC", "AG"),
    ("AE", "AE"),
    ("AF", "AF"),
    ("AG", "DZ"),
    ("AJ", "AZ"),
    ("AL", "AL"),
    ("AM", "AM"),
    ("AN", "AD"),
    ("AO", "AO"),
    ("AQ", "AS"),
    ("AR", "AR"),
    ("AS", "AU"),
    ("AU", "AT"),
    ("AV", "AI"),
    ("AY", "AQ"),
    ("BA", "BH"),
    ("BB", "BB"),
    ("BC", "BW"),
    ("BD", "BM"),
    ("BE", "BE"),
    ("BF", "BS"),
    ("BG", "BD"),
    ("BH", "BZ"),
    ("BK", "BA"),
    ("BL", "BO"),
    ("BM", "MM"),
    ("BN", "BJ"),
    ("BO", "BY"),
    ("BP", "SB"),
    ("BR", "BR"),
    ("BT", "BT"),
    ("BU", "BG"),
    ("BV", "BV"),
    ("BX", "BN"),
    ("BY", "BI"),
    ("CA", "CA"),
    ("CB", "KH"),
    ("CD", "TD"),
    ("CE", "LK"),
    ("CF", "CG"),
    ("CG", "CD"),
    ("CH", "CN"),
    ("CI", "CL"),
    ("CJ", "KY"),
    ("CK", "CC"),
    ("CM", "CM"),
    ("CN", "KM"),
    ("CO", "CO"),
    ("CS", "CR"),
    ("CT", "CF"),
    ("CU", "CU"),
    ("CV", "CV"),
    ("CW", "CK"),
    ("CY", "CY"),
    ("DA", "DK"),
    ("DJ", "DJ"),
    ("DO", "DM"),
    ("DR", "DO"),
    ("EC", "EC"),
    ("EG", "EG"),
    ("EI", "IE"),
    ("EK", "GQ"),
    ("EN", "EE"),
    ("ER", "ER"),
    ("ES", "SV"),
    ("ET", "ET"),
    ("EZ", "CZ"),
    ("FG", "GF"),
    ("FI", "FI"),
    ("FJ", "FJ"),
    ("FK", "FK"),
    ("FM", "FM"),
    ("FO", "FO"),
    ("FP", "PF"),
    ("FR", "FR"),
    ("FS", "TF"),
    ("GA", "GM"),
    ("GB", "GA"),
    ("GG", "GE"),
    ("GH", "GH"),
    ("GI", "GI"),
    ("GJ", "GD"),
    ("GK", "GG"),
    ("GL", "GL"),
    ("GM", "DE"),
    ("GP", "GP"),
    ("GQ", "GU"),
    ("GR", "GR"),
    ("GT", "GT"),
    ("GV", "GN"),
    ("GY", "GY"),
    ("GZ", "PS"),
    ("HA", "HT"),
    ("HK", "HK"),
    ("HM", "HM"),
    ("HO", "HN"),
    ("HR", "HR"),
    ("HU", "HU"),
    ("IC", "IS"),
    ("ID", "ID"),
    ("IM", "IM"),
    ("IN", "IN"),
    ("IO", "IO"),
    ("IR", "IR"),
    ("IS", "IL"),
    ("IT", "IT"),
    ("IV", "CI"),
    ("IZ", "IQ"),
    ("JA", "JP"),
    ("JE", "JE"),
    ("JM", "JM"),
    ("JO", "JO"),
    ("KE", "KE"),
    ("KG", "KG"),
    ("KN", "KP"),
    ("KR", "KI"),
    ("KS", "KR"),
    ("KT", "CX"),
    ("KU", "KW"),
    ("KV", "XK"),
    ("KZ", "KZ"),
    ("LA", "LA"),
    ("LE", "LB"),
    ("LG", "LV"),
    ("LH", "LT"),
    ("LI", "LR"),
    ("LO", "SK"),
    ("LS", "LI"),
    ("LT", "LS"),
    ("LU", "LU"),
    ("LY", "LY"),
    ("MA", "MG"),
    ("MB", "MQ"),
    ("MC", "MO"),
    ("MD", "MD"),
    ("MF", "YT"),
    ("MG", "MN"),
    ("MH", "MS"),
    ("MI", "MW"),
    ("MJ", "ME"),
    ("MK", "MK"),
    ("ML", "ML"),
    ("MN", "MC"),
    ("MO", "MA"),
    ("MP", "MU"),
    ("MR", "MR"),
    ("MT", "MT"),
    ("MU", "OM"),
    ("MV", "MV"),
    ("MX", "MX"),
    ("MY", "MY"),
    ("MZ", "MZ"),
    ("NC", "NC"),
    ("NE", "NU"),
    ("NF", "NF"),
    ("NG", "NE"),
    ("NH", "VU"),
    ("NI", "NG"),
    ("NL", "NL"),
    ("NN", "SX"),
    ("NO", "NO"),
    ("NP", "NP"),
    ("NR", "NR"),
    ("NS", "SR"),
    ("NU", "NI"),
    ("NZ", "NZ"),
    ("OD", "SS"),
    ("PA", "PY"),
    ("PC", "PN"),
    ("PE", "PE"),
    ("PK", "PK"),
    ("PL", "PL"),
    ("PM", "PA"),
    ("PO", "PT"),
    ("PP", "PG"),
    ("PS", "PW"),
    ("PU", "GW"),
    ("QA", "QA"),
    ("RE", "RE"),
    ("RI", "RS"),
    ("RM", "MH"),
    ("RN", "MF"),
    ("RO", "RO"),
    ("RP", "PH"),
    ("RQ", "PR"),
    ("RS", "RU"),
    ("RW", "RW"),
    ("SA", "SA"),
    ("SB", "PM"),
    ("SC", "KN"),
    ("SE", "SC"),
    ("SF", "ZA"),
    ("SG", "SN"),
    ("SH", "SH"),
    ("SI", "SI"),
    ("SL", "SL"),
    ("SM", "SM"),
    ("SN", "SG"),
    ("SO", "SO"),
    ("SP", "ES"),
    ("ST", "LC"),
    ("SU", "SD"),
    ("SV", "SJ"),
    ("SW", "SE"),
    ("SX", "GS"),
    ("SY", "SY"),
    ("SZ", "CH"),
    ("TB", "BL"),
    ("TD", "TT"),
    ("TH", "TH"),
    ("TI", "TJ"),
    ("TK", "TC"),
    ("TL", "TK"),
    ("TN", "TO"),
    ("TO", "TG"),
    ("TP", "ST"),
    ("TS", "TN"),
    ("TT", "TL"),
    ("TU", "TR"),
    ("TV", "TV"),
    ("TW", "TW"),
    ("TX", "TM"),
    ("TZ", "TZ"),
    ("UC", "CW"),
    ("UG", "UG"),
    ("UK", "GB"),
    ("UP", "UA"),
    ("US", "US"),
    ("UV", "BF"),
    ("UY", "UY"),
    ("UZ", "UZ"),
    ("VC", "VC"),
    ("VE", "VE"),
    ("VI", "VG"),
    ("VM", "VN"),
    ("VQ", "VI"),
    ("VT", "VA"),
    ("WA", "NA"),
    ("WE", "PS"),
    ("WF", "WF"),
    ("WI", "EH"),
    ("WS", "WS"),
    ("WZ", "SZ"),
    ("YM", "YE"),
    ("ZA", "ZM"),
    ("ZI", "ZW"),
];

/// The ISO 3166 alpha-2 code of a GDELT (FIPS 10-4) country code.
pub fn iso_of_fips(fips: &str) -> Option<&'static str> {
    FIPS_TO_ISO
        .binary_search_by(|(f, _)| f.cmp(&fips))
        .ok()
        .map(|i| FIPS_TO_ISO[i].1)
}

/// The `geo_event` kind of a code family (D739).
pub fn kind_of(code: &str) -> &'static str {
    if code.starts_with("163") {
        "sanctions"
    } else if code.starts_with("191") {
        "blockade"
    } else if code.starts_with("0871") {
        "ceasefire"
    } else if code.starts_with("18") {
        "attack"
    } else if code.starts_with("19") || code.starts_with("20") {
        "armed_conflict"
    } else {
        "other"
    }
}

// Columns of a GDELT 2.0 export line (61 fields).
const ID: usize = 0;
const DAY: usize = 1;
const ACTOR1: usize = 6;
const ACTOR2: usize = 16;
const CODE: usize = 26;
const GOLDSTEIN: usize = 30;
const SOURCES: usize = 32;
const ARTICLES: usize = 33;
const PLACE: usize = 52;
const COUNTRY: usize = 53;
const ADM1: usize = 54;
const LAT: usize = 56;
const LON: usize = 57;
const ADDED: usize = 59;
const FIELDS: usize = 61;
/// GDELT ids kept in a lead's provenance.
const IDS_MAX: usize = 50;
const ACTORS_MAX: usize = 5;

#[derive(Default)]
struct Lead {
    ids: BTreeSet<String>,
    sources: u64,
    articles: u64,
    lat: f64,
    lon: f64,
    located: u32,
    places: HashMap<String, u32>,
    regions: HashMap<String, u32>,
    actors: BTreeMap<String, u32>,
    first_added: String,
    last_added: String,
}

fn most(m: &HashMap<String, u32>) -> Option<&str> {
    // Ties go to the smallest name: the same input gives the same output.
    m.iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map(|(k, _)| k.as_str())
}

/// `YYYYMMDDHHMMSS` as RFC 3339, `None` otherwise.
fn instant(raw: &str) -> Option<String> {
    let t = chrono::NaiveDateTime::parse_from_str(raw, "%Y%m%d%H%M%S").ok()?;
    Some(t.and_utc().format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// Accumulates the lines of several files, then gives one record per lead.
#[derive(Default)]
pub struct Aggregator {
    leads: BTreeMap<(String, String, String), Lead>,
    /// Lines read, for the run's log.
    pub lines: usize,
}

impl Aggregator {
    pub fn add(&mut self, filter: &EventsFilter, text: &str) {
        for line in text.lines() {
            self.lines += 1;
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < FIELDS {
                continue;
            }
            let Some(country) = iso_of_fips(f[COUNTRY].trim()) else {
                continue;
            };
            // `*`: every country (a volume run, a world view).
            if !filter.countries.contains(country) && !filter.countries.contains("*") {
                continue;
            }
            let code = f[CODE].trim();
            let Some(family) = filter
                .codes
                .iter()
                .filter(|c| code.starts_with(c.as_str()))
                .max_by_key(|c| c.len())
            else {
                continue;
            };
            let sources: u32 = f[SOURCES].trim().parse().unwrap_or(0);
            let goldstein: f64 = f[GOLDSTEIN].trim().parse().unwrap_or(f64::MAX);
            if sources < filter.min_sources || goldstein > filter.max_goldstein {
                continue;
            }
            let Ok(day) = chrono::NaiveDate::parse_from_str(f[DAY].trim(), "%Y%m%d") else {
                continue;
            };
            let Some(added) = instant(f[ADDED].trim()) else {
                continue;
            };
            let lead = self
                .leads
                .entry((country.to_string(), family.clone(), day.to_string()))
                .or_default();
            lead.ids.insert(f[ID].trim().to_string());
            lead.sources += sources as u64;
            lead.articles += f[ARTICLES].trim().parse::<u64>().unwrap_or(0);
            let (lat, lon) = (f[LAT].trim().parse::<f64>(), f[LON].trim().parse::<f64>());
            if let (Ok(lat), Ok(lon)) = (lat, lon) {
                if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) {
                    lead.lat += lat;
                    lead.lon += lon;
                    lead.located += 1;
                }
            }
            for (col, map) in [(PLACE, &mut lead.places), (ADM1, &mut lead.regions)] {
                let v = f[col].trim();
                if !v.is_empty() {
                    *map.entry(v.to_string()).or_default() += 1;
                }
            }
            for col in [ACTOR1, ACTOR2] {
                let v = f[col].trim();
                if !v.is_empty() {
                    *lead.actors.entry(v.to_string()).or_default() += 1;
                }
            }
            if lead.first_added.is_empty() || added < lead.first_added {
                lead.first_added = added.clone();
            }
            if added > lead.last_added {
                lead.last_added = added;
            }
        }
    }

    /// One record per lead. `published_at` is the first line's addition to
    /// the feed; `known_at` the last one's: every line was public by then.
    pub fn records(self) -> Vec<Value> {
        self.leads
            .into_iter()
            .map(|((country, family, day), l)| {
                let kind = kind_of(&family);
                let place = most(&l.places).unwrap_or(&country).to_string();
                let mut actors: Vec<(String, u32)> = l.actors.into_iter().collect();
                actors.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                let actors: Vec<String> =
                    actors.into_iter().take(ACTORS_MAX).map(|a| a.0).collect();
                let location = (l.located > 0).then(
                    || json!({ "lat": l.lat / l.located as f64, "lon": l.lon / l.located as f64 }),
                );
                json!({
                    "ref": format!("gdelt:{country}:{family}:{day}"),
                    "title": format!("{place} · {kind} · {day}"),
                    "kind": kind,
                    "code": family,
                    "event_at": day,
                    "published_at": l.first_added,
                    "known_at": l.last_added,
                    "country": country,
                    "region": most(&l.regions),
                    "location": location,
                    "actors": actors.join(", "),
                    "source_count": l.sources,
                    "article_count": l.articles,
                    "gdelt_ids": l.ids.into_iter().take(IDS_MAX).collect::<Vec<_>>().join(","),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(
        id: &str,
        day: &str,
        code: &str,
        gold: &str,
        src: &str,
        country: &str,
        lat: &str,
    ) -> String {
        let mut f = vec![""; FIELDS];
        f[ID] = id;
        f[DAY] = day;
        f[ACTOR1] = "MILITARY";
        f[CODE] = code;
        f[GOLDSTEIN] = gold;
        f[SOURCES] = src;
        f[ARTICLES] = "4";
        f[PLACE] = "Kyiv, Ukraine";
        f[COUNTRY] = country;
        f[ADM1] = "UP12";
        f[LAT] = lat;
        f[LON] = "30.5";
        f[ADDED] = "20261105141500";
        f.join("\t")
    }

    fn filter() -> EventsFilter {
        EventsFilter {
            countries: ["UA".to_string()].into(),
            codes: vec!["18".into(), "19".into(), "163".into()],
            min_sources: 2,
            max_goldstein: -5.0,
        }
    }

    #[test]
    fn lines_become_one_lead_per_country_family_and_day() {
        let text = [
            line("1", "20261105", "190", "-10", "3", "UP", "50.0"),
            line("2", "20261105", "193", "-10", "5", "UP", "51.0"),
            line("3", "20261105", "183", "-9", "2", "UP", "50.0"),
            // Filtered: Russia (FIPS RS), weak sources, mild Goldstein, other code.
            line("4", "20261105", "190", "-10", "9", "RS", "55.0"),
            line("5", "20261105", "190", "-10", "1", "UP", "50.0"),
            line("6", "20261105", "190", "-2", "9", "UP", "50.0"),
            line("7", "20261105", "042", "-10", "9", "UP", "50.0"),
            "short\tline".to_string(),
        ]
        .join("\n");
        let mut agg = Aggregator::default();
        agg.add(&filter(), &text);
        let recs = agg.records();
        assert_eq!(recs.len(), 2);
        let war = recs.iter().find(|r| r["code"] == "19").unwrap();
        assert_eq!(war["ref"], "gdelt:UA:19:2026-11-05");
        assert_eq!(war["country"], "UA");
        assert_eq!(war["kind"], "armed_conflict");
        assert_eq!(war["source_count"], 8);
        assert_eq!(war["article_count"], 8);
        assert_eq!(war["gdelt_ids"], "1,2");
        assert_eq!(war["location"]["lat"], 50.5);
        assert_eq!(war["region"], "UP12");
        assert_eq!(war["known_at"], "2026-11-05T14:15:00Z");
        let attack = recs.iter().find(|r| r["code"] == "18").unwrap();
        assert_eq!(attack["kind"], "attack");
    }

    #[test]
    fn fips_codes_become_iso() {
        for (fips, iso) in [
            ("GM", "DE"),
            ("UK", "GB"),
            ("UP", "UA"),
            ("IS", "IL"),
            ("SZ", "CH"),
            ("CH", "CN"),
            ("RS", "RU"),
            ("WE", "PS"),
            ("FR", "FR"),
        ] {
            assert_eq!(iso_of_fips(fips), Some(iso), "{fips}");
        }
        assert_eq!(iso_of_fips("OS"), None);
        assert!(
            FIPS_TO_ISO.windows(2).all(|w| w[0].0 < w[1].0),
            "sorted for the search"
        );
    }

    #[test]
    fn the_longest_family_wins() {
        let f = EventsFilter {
            codes: vec!["16".into(), "163".into()],
            ..filter()
        };
        let mut agg = Aggregator::default();
        agg.add(&f, &line("1", "20261105", "1631", "-8", "3", "UP", "50"));
        let recs = agg.records();
        assert_eq!(recs[0]["kind"], "sanctions");
        assert_eq!(recs[0]["code"], "163");
    }

    #[test]
    fn names_cover_the_window() {
        let end = chrono::DateTime::parse_from_rfc3339("2026-11-05T10:07:00Z")
            .unwrap()
            .to_utc();
        let n = file_names(end, 1);
        assert_eq!(n.len(), 4);
        assert_eq!(n[0], "20261105100000.export.CSV.zip");
        assert_eq!(n[3], "20261105091500.export.CSV.zip");
    }
}
