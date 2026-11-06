//! Verified conflict events (markets-research.md D741): UCDP (Uppsala,
//! CC BY 4.0) and ACLED (the organization's own account and licence). Each
//! event becomes a confirmed `geo_event`; a GDELT lead of the same country,
//! within a day and of a compatible kind, is then confirmed by it. Pure: the
//! server fetches the pages.

use serde_json::{json, Value};

/// Where verified events come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConflictSource {
    /// UCDP's API (live, token).
    Ucdp,
    /// UCDP's downloads (GED history, Candidate months): CSV, plain or
    /// zipped, no token.
    UcdpFile,
    Acled,
}

impl ConflictSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ucdp | Self::UcdpFile => "ucdp",
            Self::Acled => "acled",
        }
    }

    /// The only host the source's credentials go to (R9).
    pub fn host(self) -> &'static str {
        match self {
            Self::Ucdp => "ucdpapi.pcr.uu.se",
            Self::UcdpFile => "ucdp.uu.se",
            Self::Acled => "acleddata.com",
        }
    }

    /// Vault secrets of the org: UCDP's access token; ACLED's account
    /// e-mail and password (the org holds its own ACLED licence).
    pub fn secrets(self) -> &'static [&'static str] {
        match self {
            Self::Ucdp => &["world_events_ucdp_token"],
            Self::UcdpFile => &[],
            Self::Acled => &["world_events_acled_email", "world_events_acled_password"],
        }
    }
}

/// Gleditsch & Ward state numbers (UCDP's `country_id`) → ISO 3166 alpha-2.
// shortcut: states with a UCDP record since 1989 and their neighbours; an
// unknown number is left out and counted in the run's log. Add rows as needed.
const GW_TO_ISO: [(u16, &str); 175] = [
    (2, "US"),
    (20, "CA"),
    (31, "BS"),
    (40, "CU"),
    (41, "HT"),
    (42, "DO"),
    (51, "JM"),
    (52, "TT"),
    (53, "BB"),
    (70, "MX"),
    (80, "BZ"),
    (90, "GT"),
    (91, "HN"),
    (92, "SV"),
    (93, "NI"),
    (94, "CR"),
    (95, "PA"),
    (100, "CO"),
    (101, "VE"),
    (110, "GY"),
    (115, "SR"),
    (130, "EC"),
    (135, "PE"),
    (140, "BR"),
    (145, "BO"),
    (150, "PY"),
    (155, "CL"),
    (160, "AR"),
    (165, "UY"),
    (200, "GB"),
    (205, "IE"),
    (210, "NL"),
    (211, "BE"),
    (212, "LU"),
    (220, "FR"),
    (225, "CH"),
    (230, "ES"),
    (235, "PT"),
    (260, "DE"),
    (290, "PL"),
    (305, "AT"),
    (310, "HU"),
    (316, "CZ"),
    (317, "SK"),
    (325, "IT"),
    (338, "MT"),
    (339, "AL"),
    (340, "RS"),
    (341, "ME"),
    (343, "MK"),
    (344, "HR"),
    // UCDP codes Serbia, then Yugoslavia, as 345 (GED 26.1).
    (345, "RS"),
    (346, "BA"),
    (347, "XK"),
    (349, "SI"),
    (350, "GR"),
    (352, "CY"),
    (355, "BG"),
    (359, "MD"),
    (360, "RO"),
    (365, "RU"),
    (366, "EE"),
    (367, "LV"),
    (368, "LT"),
    (369, "UA"),
    (370, "BY"),
    (371, "AM"),
    (372, "GE"),
    (373, "AZ"),
    (375, "FI"),
    (380, "SE"),
    (385, "NO"),
    (390, "DK"),
    (395, "IS"),
    (402, "CV"),
    (404, "GW"),
    (411, "GQ"),
    (420, "GM"),
    (432, "ML"),
    (433, "SN"),
    (434, "BJ"),
    (435, "MR"),
    (436, "NE"),
    (437, "CI"),
    (438, "GN"),
    (439, "BF"),
    (450, "LR"),
    (451, "SL"),
    (452, "GH"),
    (461, "TG"),
    (471, "CM"),
    (475, "NG"),
    (481, "GA"),
    (482, "CF"),
    (483, "TD"),
    (484, "CG"),
    (490, "CD"),
    (500, "UG"),
    (501, "KE"),
    (510, "TZ"),
    (516, "BI"),
    (517, "RW"),
    (520, "SO"),
    (522, "DJ"),
    (530, "ET"),
    (531, "ER"),
    (540, "AO"),
    (541, "MZ"),
    (551, "ZM"),
    (552, "ZW"),
    (553, "MW"),
    (560, "ZA"),
    (565, "NA"),
    (570, "LS"),
    (571, "BW"),
    (572, "SZ"),
    (580, "MG"),
    (581, "KM"),
    (590, "MU"),
    (600, "MA"),
    (615, "DZ"),
    (616, "TN"),
    (620, "LY"),
    (625, "SD"),
    (626, "SS"),
    (630, "IR"),
    (640, "TR"),
    (645, "IQ"),
    (651, "EG"),
    (652, "SY"),
    (660, "LB"),
    (663, "JO"),
    (666, "IL"),
    (670, "SA"),
    (678, "YE"),
    (690, "KW"),
    (692, "BH"),
    (694, "QA"),
    (696, "AE"),
    (698, "OM"),
    (700, "AF"),
    (701, "TM"),
    (702, "TJ"),
    (703, "KG"),
    (704, "UZ"),
    (705, "KZ"),
    (710, "CN"),
    (712, "MN"),
    (713, "TW"),
    (731, "KP"),
    (732, "KR"),
    (740, "JP"),
    (750, "IN"),
    (760, "BT"),
    (770, "PK"),
    (771, "BD"),
    (775, "MM"),
    (780, "LK"),
    (781, "MV"),
    (790, "NP"),
    (800, "TH"),
    (811, "KH"),
    (812, "LA"),
    (816, "VN"),
    (820, "MY"),
    (830, "SG"),
    (835, "BN"),
    (840, "PH"),
    (850, "ID"),
    (860, "TL"),
    (900, "AU"),
    (910, "PG"),
    (920, "NZ"),
    (940, "SB"),
    (950, "FJ"),
];

/// ISO 3166 numeric (ACLED's `iso`) → alpha-2.
const ISO_NUMERIC: [(u16, &str); 249] = [
    (4, "AF"),
    (8, "AL"),
    (10, "AQ"),
    (12, "DZ"),
    (16, "AS"),
    (20, "AD"),
    (24, "AO"),
    (28, "AG"),
    (31, "AZ"),
    (32, "AR"),
    (36, "AU"),
    (40, "AT"),
    (44, "BS"),
    (48, "BH"),
    (50, "BD"),
    (51, "AM"),
    (52, "BB"),
    (56, "BE"),
    (60, "BM"),
    (64, "BT"),
    (68, "BO"),
    (70, "BA"),
    (72, "BW"),
    (74, "BV"),
    (76, "BR"),
    (84, "BZ"),
    (86, "IO"),
    (90, "SB"),
    (92, "VG"),
    (96, "BN"),
    (100, "BG"),
    (104, "MM"),
    (108, "BI"),
    (112, "BY"),
    (116, "KH"),
    (120, "CM"),
    (124, "CA"),
    (132, "CV"),
    (136, "KY"),
    (140, "CF"),
    (144, "LK"),
    (148, "TD"),
    (152, "CL"),
    (156, "CN"),
    (158, "TW"),
    (162, "CX"),
    (166, "CC"),
    (170, "CO"),
    (174, "KM"),
    (175, "YT"),
    (178, "CG"),
    (180, "CD"),
    (184, "CK"),
    (188, "CR"),
    (191, "HR"),
    (192, "CU"),
    (196, "CY"),
    (203, "CZ"),
    (204, "BJ"),
    (208, "DK"),
    (212, "DM"),
    (214, "DO"),
    (218, "EC"),
    (222, "SV"),
    (226, "GQ"),
    (231, "ET"),
    (232, "ER"),
    (233, "EE"),
    (234, "FO"),
    (238, "FK"),
    (239, "GS"),
    (242, "FJ"),
    (246, "FI"),
    (248, "AX"),
    (250, "FR"),
    (254, "GF"),
    (258, "PF"),
    (260, "TF"),
    (262, "DJ"),
    (266, "GA"),
    (268, "GE"),
    (270, "GM"),
    (275, "PS"),
    (276, "DE"),
    (288, "GH"),
    (292, "GI"),
    (296, "KI"),
    (300, "GR"),
    (304, "GL"),
    (308, "GD"),
    (312, "GP"),
    (316, "GU"),
    (320, "GT"),
    (324, "GN"),
    (328, "GY"),
    (332, "HT"),
    (334, "HM"),
    (336, "VA"),
    (340, "HN"),
    (344, "HK"),
    (348, "HU"),
    (352, "IS"),
    (356, "IN"),
    (360, "ID"),
    (364, "IR"),
    (368, "IQ"),
    (372, "IE"),
    (376, "IL"),
    (380, "IT"),
    (384, "CI"),
    (388, "JM"),
    (392, "JP"),
    (398, "KZ"),
    (400, "JO"),
    (404, "KE"),
    (408, "KP"),
    (410, "KR"),
    (414, "KW"),
    (417, "KG"),
    (418, "LA"),
    (422, "LB"),
    (426, "LS"),
    (428, "LV"),
    (430, "LR"),
    (434, "LY"),
    (438, "LI"),
    (440, "LT"),
    (442, "LU"),
    (446, "MO"),
    (450, "MG"),
    (454, "MW"),
    (458, "MY"),
    (462, "MV"),
    (466, "ML"),
    (470, "MT"),
    (474, "MQ"),
    (478, "MR"),
    (480, "MU"),
    (484, "MX"),
    (492, "MC"),
    (496, "MN"),
    (498, "MD"),
    (499, "ME"),
    (500, "MS"),
    (504, "MA"),
    (508, "MZ"),
    (512, "OM"),
    (516, "NA"),
    (520, "NR"),
    (524, "NP"),
    (528, "NL"),
    (531, "CW"),
    (533, "AW"),
    (534, "SX"),
    (535, "BQ"),
    (540, "NC"),
    (548, "VU"),
    (554, "NZ"),
    (558, "NI"),
    (562, "NE"),
    (566, "NG"),
    (570, "NU"),
    (574, "NF"),
    (578, "NO"),
    (580, "MP"),
    (581, "UM"),
    (583, "FM"),
    (584, "MH"),
    (585, "PW"),
    (586, "PK"),
    (591, "PA"),
    (598, "PG"),
    (600, "PY"),
    (604, "PE"),
    (608, "PH"),
    (612, "PN"),
    (616, "PL"),
    (620, "PT"),
    (624, "GW"),
    (626, "TL"),
    (630, "PR"),
    (634, "QA"),
    (638, "RE"),
    (642, "RO"),
    (643, "RU"),
    (646, "RW"),
    (652, "BL"),
    (654, "SH"),
    (659, "KN"),
    (660, "AI"),
    (662, "LC"),
    (663, "MF"),
    (666, "PM"),
    (670, "VC"),
    (674, "SM"),
    (678, "ST"),
    (682, "SA"),
    (686, "SN"),
    (688, "RS"),
    (690, "SC"),
    (694, "SL"),
    (702, "SG"),
    (703, "SK"),
    (704, "VN"),
    (705, "SI"),
    (706, "SO"),
    (710, "ZA"),
    (716, "ZW"),
    (724, "ES"),
    (728, "SS"),
    (729, "SD"),
    (732, "EH"),
    (740, "SR"),
    (744, "SJ"),
    (748, "SZ"),
    (752, "SE"),
    (756, "CH"),
    (760, "SY"),
    (762, "TJ"),
    (764, "TH"),
    (768, "TG"),
    (772, "TK"),
    (776, "TO"),
    (780, "TT"),
    (784, "AE"),
    (788, "TN"),
    (792, "TR"),
    (795, "TM"),
    (796, "TC"),
    (798, "TV"),
    (800, "UG"),
    (804, "UA"),
    (807, "MK"),
    (818, "EG"),
    (826, "GB"),
    (831, "GG"),
    (832, "JE"),
    (833, "IM"),
    (834, "TZ"),
    (840, "US"),
    (850, "VI"),
    (854, "BF"),
    (858, "UY"),
    (860, "UZ"),
    (862, "VE"),
    (876, "WF"),
    (882, "WS"),
    (887, "YE"),
    (894, "ZM"),
];

fn lookup(table: &[(u16, &'static str)], n: u16) -> Option<&'static str> {
    table
        .binary_search_by_key(&n, |(k, _)| *k)
        .ok()
        .map(|i| table[i].1)
}

pub fn iso_of_gw(n: u16) -> Option<&'static str> {
    lookup(&GW_TO_ISO, n)
}

pub fn iso_of_numeric(n: u16) -> Option<&'static str> {
    lookup(&ISO_NUMERIC, n)
}

/// The numeric ISO code of an alpha-2 one (ACLED's `iso` filter).
pub fn numeric_of_iso(alpha2: &str) -> Option<u16> {
    ISO_NUMERIC
        .iter()
        .find(|(_, a)| a.eq_ignore_ascii_case(alpha2))
        .map(|(n, _)| *n)
}

fn num(v: &Value, k: &str) -> Option<f64> {
    match v.get(k)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn text(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn day(raw: &str) -> Option<String> {
    let d = raw.get(..10)?;
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
        .ok()
        .map(|d| d.to_string())
}

/// One verified event, whichever the source, before aggregation.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: String,
    pub country: &'static str,
    pub kind: &'static str,
    pub day: chrono::NaiveDate,
    pub place: String,
    pub region: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub actors: Vec<String>,
    pub sources: f64,
    pub deaths: f64,
    /// The first day a source reported it (UCDP's `source_date`), when given.
    pub reported: Option<chrono::NaiveDate>,
}

/// The rows of a UCDP API page (`{TotalPages, Result: […]}`) and its page
/// count. A CSV download has the same columns, one row per record.
pub fn ucdp_rows(doc: &Value) -> Option<(&Vec<Value>, u64)> {
    Some((
        doc.get("Result")?.as_array()?,
        doc.get("TotalPages")?.as_u64()?,
    ))
}

/// The rows of an ACLED page (`{data: […]}`).
pub fn acled_rows(doc: &Value) -> Option<&Vec<Value>> {
    doc.get("data")?.as_array()
}

/// A UCDP row (API or CSV, values as numbers or text).
pub fn ucdp_event(r: &Value) -> Option<Event> {
    let id = text(r, "id");
    let country = iso_of_gw(num(r, "country_id")? as u16)?;
    let day = chrono::NaiveDate::parse_from_str(&day(&text(r, "date_start"))?, "%Y-%m-%d").ok()?;
    if id.is_empty() {
        return None;
    }
    // 1 state-based, 2 non-state: fighting; 3 one-sided: on civilians.
    let kind = match num(r, "type_of_violence")? as u8 {
        1 | 2 => "armed_conflict",
        3 => "attack",
        _ => return None,
    };
    // `source_date`: the dates of the articles, `;`-separated.
    let reported = text(r, "source_date")
        .split(';')
        .filter_map(|d| chrono::NaiveDate::parse_from_str(d.trim().get(..10)?, "%Y-%m-%d").ok())
        .min();
    Some(Event {
        id,
        country,
        kind,
        day,
        place: text(r, "where_coordinates"),
        region: text(r, "adm_1"),
        lat: num(r, "latitude"),
        lon: num(r, "longitude"),
        actors: vec![text(r, "side_a"), text(r, "side_b")],
        sources: num(r, "number_of_sources").unwrap_or(0.0),
        deaths: num(r, "best").unwrap_or(0.0),
        reported,
    })
}

/// An ACLED row. Its `timestamp` is ACLED's last change, not a first report.
pub fn acled_event(r: &Value) -> Option<Event> {
    let id = text(r, "event_id_cnty");
    let country = iso_of_numeric(num(r, "iso")? as u16)?;
    let day = chrono::NaiveDate::parse_from_str(&day(&text(r, "event_date"))?, "%Y-%m-%d").ok()?;
    if id.is_empty() {
        return None;
    }
    let kind = match (
        text(r, "event_type").as_str(),
        text(r, "sub_event_type").as_str(),
    ) {
        ("Battles", _) => "armed_conflict",
        ("Explosions/Remote violence" | "Violence against civilians", _) => "attack",
        ("Strategic developments", "Agreement") => "ceasefire",
        _ => "other",
    };
    Some(Event {
        id,
        country,
        kind,
        day,
        place: text(r, "location"),
        region: text(r, "admin1"),
        lat: num(r, "latitude"),
        lon: num(r, "longitude"),
        actors: vec![text(r, "actor1"), text(r, "actor2")],
        sources: 0.0,
        deaths: num(r, "fatalities").unwrap_or(0.0),
        reported: None,
    })
}

/// Ids kept in a verified event's provenance.
const IDS_MAX: usize = 50;

#[derive(Default)]
struct Day {
    ids: Vec<String>,
    places: std::collections::HashMap<String, u32>,
    regions: std::collections::HashMap<String, u32>,
    actors: std::collections::BTreeMap<String, u32>,
    lat: f64,
    lon: f64,
    located: u32,
    sources: f64,
    deaths: f64,
    reported: Option<chrono::NaiveDate>,
}

fn most(m: &std::collections::HashMap<String, u32>) -> String {
    m.iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map(|(k, _)| k.clone())
        .unwrap_or_default()
}

/// Verified events become one per (country, kind, day): a war gives tens
/// of thousands of events, the comparison with leads and the tests work by
/// day (D741). Only the `countries` kept (ISO alpha-2, upper case).
pub struct Aggregator {
    countries: std::collections::BTreeSet<String>,
    days: std::collections::BTreeMap<(&'static str, &'static str, chrono::NaiveDate), Day>,
    /// Rows seen and kept, for the run's log.
    pub seen: usize,
    pub kept: usize,
}

impl Aggregator {
    pub fn new(countries: &[String]) -> Self {
        Self {
            countries: countries.iter().map(|c| c.trim().to_uppercase()).collect(),
            days: Default::default(),
            seen: 0,
            kept: 0,
        }
    }

    pub fn add(&mut self, e: Option<Event>) {
        self.seen += 1;
        let all = self.countries.contains("*");
        let Some(e) = e.filter(|e| all || self.countries.contains(e.country)) else {
            return;
        };
        self.kept += 1;
        let d = self.days.entry((e.country, e.kind, e.day)).or_default();
        if d.ids.len() < IDS_MAX {
            d.ids.push(e.id);
        }
        for (v, m) in [(e.place, &mut d.places), (e.region, &mut d.regions)] {
            if !v.is_empty() {
                *m.entry(v).or_default() += 1;
            }
        }
        for a in e.actors.into_iter().filter(|a| !a.is_empty()) {
            *d.actors.entry(a).or_default() += 1;
        }
        if let (Some(lat), Some(lon)) = (e.lat, e.lon) {
            if lat.abs() <= 90.0 && lon.abs() <= 180.0 {
                d.lat += lat;
                d.lon += lon;
                d.located += 1;
            }
        }
        d.sources += e.sources;
        d.deaths += e.deaths;
        d.reported = match (d.reported, e.reported) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }

    /// One record per day. `known_at` is the end of the first day a source
    /// reported one of its events; without any, `fallback` (the reading's
    /// instant for a live API), else none: a history without report dates
    /// stays out of the tests that read the past (D531).
    pub fn records(self, source: &str, fallback: Option<&str>) -> Vec<Value> {
        self.days
            .into_iter()
            .map(|((country, kind, day), d)| {
                let place = most(&d.places);
                let mut actors: Vec<(String, u32)> = d.actors.into_iter().collect();
                actors.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                let actors: Vec<String> = actors.into_iter().take(5).map(|a| a.0).collect();
                let known_at = d
                    .reported
                    .map(|r| format!("{r}T23:59:59Z"))
                    .or(fallback.map(str::to_string));
                json!({
                    "ref": format!("{source}:{country}:{kind}:{day}"),
                    "title": format!("{} · {kind} · {day}", if place.is_empty() { country } else { &place }),
                    "kind": kind,
                    "event_at": day.to_string(),
                    "known_at": known_at,
                    "country": country,
                    "region": most(&d.regions),
                    "location": (d.located > 0).then(|| json!({
                        "lat": d.lat / d.located as f64,
                        "lon": d.lon / d.located as f64,
                    })),
                    "actors": actors.join(", "),
                    "source_count": d.sources,
                    "deaths": d.deaths,
                    "event_ids": d.ids.join(","),
                })
            })
            .collect()
    }
}

/// Kinds that confirm each other: fighting and attacks are coded either way
/// by different sources; the others must be the same.
fn compatible(a: &str, b: &str) -> bool {
    let violent = |k: &str| matches!(k, "armed_conflict" | "attack");
    a == b || (violent(a) && violent(b))
}

/// A lead as the confirmation pass sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Dated {
    pub country: String,
    pub day: chrono::NaiveDate,
    pub kind: String,
}

/// Does `verified` confirm `lead` (D741): same country, at most one day
/// apart, compatible kind.
pub fn confirms(verified: &Dated, lead: &Dated) -> bool {
    verified.country.eq_ignore_ascii_case(&lead.country)
        && (verified.day - lead.day).num_days().abs() <= 1
        && compatible(&verified.kind, &lead.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_and_right_on_known_codes() {
        assert!(GW_TO_ISO.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(ISO_NUMERIC.windows(2).all(|w| w[0].0 < w[1].0));
        for (gw, iso) in [
            (345, "RS"),
            (2, "US"),
            (260, "DE"),
            (365, "RU"),
            (369, "UA"),
            (666, "IL"),
            (630, "IR"),
            (678, "YE"),
            (490, "CD"),
            (775, "MM"),
        ] {
            assert_eq!(iso_of_gw(gw), Some(iso), "{gw}");
        }
        for (n, iso) in [
            (804, "UA"),
            (643, "RU"),
            (376, "IL"),
            (364, "IR"),
            (682, "SA"),
            (840, "US"),
            (826, "GB"),
            (276, "DE"),
            (275, "PS"),
            (887, "YE"),
        ] {
            assert_eq!(iso_of_numeric(n), Some(iso), "{n}");
        }
        assert_eq!(numeric_of_iso("ua"), Some(804));
        assert_eq!(iso_of_gw(1), None);
    }

    #[test]
    fn ucdp_rows_become_one_record_per_country_kind_and_day() {
        let doc = json!({"TotalPages": 3, "Result": [
            {"id": 101, "country_id": 369, "date_start": "2026-11-04 00:00:00.000", "type_of_violence": 1,
             "where_coordinates": "Kharkiv city", "adm_1": "Kharkiv oblast", "latitude": 50.0, "longitude": 36.0,
             "side_a": "Government of Russia (Soviet Union)", "side_b": "Government of Ukraine",
             "number_of_sources": 3, "best": 4, "source_date": "2026-11-05;2026-11-04"},
            // Same day and kind, as a CSV gives it (text values).
            {"id": "102", "country_id": "369", "date_start": "2026-11-04", "type_of_violence": "2",
             "latitude": "52.0", "longitude": "38.0", "number_of_sources": "1", "best": "6", "source_date": "2026-11-06"},
            {"id": 103, "country_id": 365, "date_start": "2026-11-04", "type_of_violence": 1},
            {"id": 104, "country_id": 9999, "date_start": "2026-11-04", "type_of_violence": 1},
            {"id": 105, "country_id": 369, "date_start": "bad", "type_of_violence": 3}
        ]});
        let (rows, pages) = ucdp_rows(&doc).unwrap();
        assert_eq!(pages, 3);
        let mut agg = Aggregator::new(&["ua".to_string()]);
        rows.iter().for_each(|r| agg.add(ucdp_event(r)));
        assert_eq!((agg.seen, agg.kept), (5, 2));
        let recs = agg.records("ucdp", None);
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r["ref"], "ucdp:UA:armed_conflict:2026-11-04");
        assert_eq!(r["deaths"], 10.0);
        assert_eq!(r["source_count"], 4.0);
        assert_eq!(r["location"]["lat"], 51.0);
        assert_eq!(r["known_at"], "2026-11-04T23:59:59Z", "the first report");
        assert_eq!(r["event_ids"], "101,102");
        assert!(ucdp_rows(&json!({"Result": []})).is_none());
    }

    #[test]
    fn a_history_without_report_dates_has_no_known_at() {
        let row =
            json!({"id": 1, "country_id": 369, "date_start": "2015-02-01", "type_of_violence": 3});
        let mut agg = Aggregator::new(&["UA".to_string()]);
        agg.add(ucdp_event(&row));
        assert!(agg.records("ucdp", None)[0]["known_at"].is_null());
        let mut agg = Aggregator::new(&["UA".to_string()]);
        agg.add(ucdp_event(&row));
        assert_eq!(
            agg.records("ucdp", Some("2026-11-06T03:00:00Z"))[0]["known_at"],
            "2026-11-06T03:00:00Z"
        );
    }

    #[test]
    fn acled_rows_become_records() {
        let doc = json!({"status": 200, "data": [
            {"event_id_cnty": "UKR123", "event_date": "2026-11-05", "event_type": "Explosions/Remote violence",
             "sub_event_type": "Air/drone strike", "iso": "804", "admin1": "Kyiv", "location": "Kyiv",
             "latitude": "50.45", "longitude": "30.52", "actor1": "Military Forces of Russia", "fatalities": "2"},
            {"event_id_cnty": "XXX1", "event_date": "2026-11-05", "event_type": "Battles", "iso": 0}
        ]});
        let mut agg = Aggregator::new(&["UA".to_string()]);
        acled_rows(&doc)
            .unwrap()
            .iter()
            .for_each(|r| agg.add(acled_event(r)));
        let recs = agg.records("acled", Some("2026-11-06T03:00:00Z"));
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0]["kind"], "attack");
        assert_eq!(recs[0]["ref"], "acled:UA:attack:2026-11-05");
        assert_eq!(recs[0]["location"]["lat"], 50.45);
    }

    #[test]
    fn confirmation_needs_country_day_and_kind() {
        let d = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let at = |c: &str, day: &str, k: &str| Dated {
            country: c.into(),
            day: d(day),
            kind: k.into(),
        };
        let v = at("UA", "2026-11-05", "armed_conflict");
        assert!(confirms(&v, &at("ua", "2026-11-04", "attack")));
        assert!(!confirms(&v, &at("UA", "2026-11-03", "armed_conflict")));
        assert!(!confirms(&v, &at("RU", "2026-11-05", "armed_conflict")));
        assert!(!confirms(&v, &at("UA", "2026-11-05", "sanctions")));
    }
}
