//! CSV / iCalendar import of time ranges (media-ingest.md D169): small
//! parsers written here (no new dependency), then the shared upsert keyed
//! by `external_id` (CSV column, ICS `UID`) — re-importing a schedule
//! updates it. Rows are checked one by one; a faulty row is counted with
//! its line, field and machine token, the others go through.
//!
//! - CSV: header row required (`label` mandatory; `external_id`,
//!   `category`, `planned_start`, `planned_end`, `actual_start`,
//!   `actual_end`, `source_url`, `confidence` read when present, other
//!   columns ignored), `,` or `;` separator, RFC 4180 quoting, RFC 3339
//!   timestamps, an empty cell leaves the field as it is.
//! - ICS: `VEVENT`s of a `VCALENDAR`: `UID`, `SUMMARY`, `DTSTART`, `DTEND`
//!   or `DURATION`, `CATEGORIES` (first), `URL`, `DESCRIPTION` (in
//!   `attrs.description`); UTC, `TZID` and all-day dates. Times are planned.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use pnex_core::err_codes;
use pnex_core::time_range::{
    RangeImportError, RangeImportResult, RangeOrigin, TimeRangeInput, IMPORT_ERRORS_MAX,
    IMPORT_MAX_ROWS,
};
use sea_orm::ConnectionTrait;

use super::{upsert, RangeError, Scope, Writer};

/// Longest `DESCRIPTION` kept (characters), so `attrs` stays bounded.
const DESCRIPTION_MAX: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Ics,
}

impl Format {
    pub fn from_wire(s: &str) -> Option<Self> {
        match s.trim() {
            "csv" => Some(Self::Csv),
            "ics" => Some(Self::Ics),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Not a readable file of the announced format.
    Invalid,
    /// More than [`IMPORT_MAX_ROWS`] rows.
    TooManyRows,
}

/// One parsed row with the line where it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub line: u32,
    pub input: TimeRangeInput,
}

pub fn parse(format: Format, body: &[u8]) -> Result<Vec<Row>, ParseError> {
    let text = std::str::from_utf8(body).map_err(|_| ParseError::Invalid)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match format {
        Format::Csv => parse_csv(text),
        Format::Ics => parse_ics(text),
    }
}

// ─────────────────────────────── CSV ───────────────────────────────

/// RFC 4180 records with their starting line; quoted fields may hold the
/// separator, `""` and line breaks.
fn csv_records(text: &str, sep: char) -> Vec<(u32, Vec<String>)> {
    let mut out = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut line = 1u32;
    let mut start = 1u32;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    field.push(c);
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            '\r' => {}
            '\n' => {
                record.push(std::mem::take(&mut field));
                if record.iter().any(|f| !f.trim().is_empty()) {
                    out.push((start, std::mem::take(&mut record)));
                }
                record.clear();
                line += 1;
                start = line;
            }
            c if c == sep => record.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    record.push(field);
    if record.iter().any(|f| !f.trim().is_empty()) {
        out.push((start, record));
    }
    out
}

fn parse_csv(text: &str) -> Result<Vec<Row>, ParseError> {
    let first = text.lines().next().unwrap_or_default();
    let sep = if first.contains(';') && !first.contains(',') {
        ';'
    } else {
        ','
    };
    let mut records = csv_records(text, sep).into_iter();
    let (_, header) = records.next().ok_or(ParseError::Invalid)?;
    let header: Vec<String> = header.iter().map(|h| h.trim().to_lowercase()).collect();
    let col = |name: &str| header.iter().position(|h| h == name);
    if col("label").is_none() {
        return Err(ParseError::Invalid);
    }
    let mut rows = Vec::new();
    for (line, rec) in records {
        if rows.len() == IMPORT_MAX_ROWS {
            return Err(ParseError::TooManyRows);
        }
        // An empty cell is an absent field (keeps the stored value).
        let cell = |name: &str| {
            col(name)
                .and_then(|i| rec.get(i))
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let input = TimeRangeInput {
            label: Some(cell("label").unwrap_or_default()),
            external_id: cell("external_id"),
            category: cell("category"),
            planned_start: cell("planned_start"),
            planned_end: cell("planned_end"),
            actual_start: cell("actual_start"),
            actual_end: cell("actual_end"),
            source_url: cell("source_url"),
            // Unreadable → NaN, refused by the shared check as `invalid`.
            confidence: cell("confidence").map(|c| c.parse().unwrap_or(f64::NAN)),
            ..Default::default()
        };
        rows.push(Row { line, input });
    }
    Ok(rows)
}

// ─────────────────────────────── ICS ───────────────────────────────

/// Unfolded content lines (RFC 5545 §3.1) with their first physical line.
fn unfold(text: &str) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match (raw.strip_prefix([' ', '\t']), out.last_mut()) {
            (Some(rest), Some((_, last))) => last.push_str(rest),
            _ if raw.is_empty() => {}
            _ => out.push((i as u32 + 1, raw.to_string())),
        }
    }
    out
}

/// `(NAME, VALUE)` parameters of a content line.
type Params = Vec<(String, String)>;

/// `NAME;P=V;…:value` → (upper-case name, params, value). Quoted parameter
/// values may hold `:` and `;`.
fn property(line: &str) -> Option<(String, Params, String)> {
    let mut quoted = false;
    let colon = line.char_indices().find_map(|(i, c)| {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => return Some(i),
            _ => {}
        }
        None
    })?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| {
            (
                k.trim().to_ascii_uppercase(),
                v.trim_matches('"').to_string(),
            )
        })
        .collect();
    Some((name, params, value.to_string()))
}

fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// ICS date or date-time → RFC 3339. Unreadable values are returned as
/// they are: the shared check then refuses the field as `invalid`.
fn ics_time(value: &str, params: &[(String, String)]) -> String {
    let v = value.trim();
    let param = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    if param("VALUE") == Some("DATE") || v.len() == 8 {
        return NaiveDate::parse_from_str(v, "%Y%m%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| Utc.from_utc_datetime(&d).to_rfc3339())
            .unwrap_or_else(|| v.to_string());
    }
    let (local, utc) = match v.strip_suffix('Z') {
        Some(rest) => (rest, true),
        None => (v, false),
    };
    let Ok(naive) = NaiveDateTime::parse_from_str(local, "%Y%m%dT%H%M%S") else {
        return v.to_string();
    };
    if utc {
        return Utc.from_utc_datetime(&naive).to_rfc3339();
    }
    match param("TZID") {
        Some(tz) => tz
            .parse::<chrono_tz::Tz>()
            .ok()
            .and_then(|tz| tz.from_local_datetime(&naive).earliest())
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| v.to_string()),
        // shortcut: floating times are read as UTC; upgrade if a real
        // schedule ships floating times (a per-import timezone then).
        None => Utc.from_utc_datetime(&naive).to_rfc3339(),
    }
}

/// `DURATION` (`P1DT2H30M`, `PT45M`, `P1W`) in seconds.
fn ics_duration(v: &str) -> Option<i64> {
    let rest = v
        .trim()
        .strip_prefix('+')
        .unwrap_or(v.trim())
        .strip_prefix('P')?;
    let (mut total, mut num, mut time) = (0i64, String::new(), false);
    for c in rest.chars() {
        match c {
            '0'..='9' => num.push(c),
            'T' => time = true,
            unit => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += n * match (unit, time) {
                    ('W', false) => 7 * 86_400,
                    ('D', false) => 86_400,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    (num.is_empty() && total > 0).then_some(total)
}

fn parse_ics(text: &str) -> Result<Vec<Row>, ParseError> {
    let lines = unfold(text);
    if !lines
        .iter()
        .any(|(_, l)| l.trim().eq_ignore_ascii_case("BEGIN:VCALENDAR"))
    {
        return Err(ParseError::Invalid);
    }
    let mut rows = Vec::new();
    // (line of BEGIN:VEVENT, input, DURATION) of the open event.
    let mut event: Option<(u32, TimeRangeInput, Option<String>)> = None;
    // Depth of components nested in the event (VALARM…), skipped.
    let mut nested = 0usize;
    for (line, l) in &lines {
        let Some((name, params, value)) = property(l) else {
            continue;
        };
        let upper = value.trim().to_ascii_uppercase();
        match (name.as_str(), event.as_mut()) {
            ("BEGIN", None) if upper == "VEVENT" => {
                if rows.len() == IMPORT_MAX_ROWS {
                    return Err(ParseError::TooManyRows);
                }
                event = Some((*line, TimeRangeInput::default(), None));
            }
            ("BEGIN", Some(_)) => nested += 1,
            ("END", Some(_)) if nested > 0 => nested -= 1,
            ("END", Some(_)) if upper == "VEVENT" => {
                let (start_line, mut input, duration) = event.take().expect("open event");
                if input.planned_end.is_none() {
                    let start = input
                        .planned_start
                        .as_deref()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok());
                    if let (Some(s), Some(d)) = (start, duration.as_deref().and_then(ics_duration))
                    {
                        input.planned_end = Some((s + chrono::Duration::seconds(d)).to_rfc3339());
                    }
                }
                input.label.get_or_insert_with(String::new);
                rows.push(Row {
                    line: start_line,
                    input,
                });
            }
            (_, Some(_)) if nested > 0 => {}
            (_, Some((_, input, duration))) => match name.as_str() {
                "UID" => input.external_id = Some(unescape(&value)),
                "SUMMARY" => input.label = Some(unescape(&value)),
                "DTSTART" => input.planned_start = Some(ics_time(&value, &params)),
                "DTEND" => input.planned_end = Some(ics_time(&value, &params)),
                "DURATION" => *duration = Some(value.clone()),
                "CATEGORIES" => {
                    input.category = unescape(&value)
                        .split(',')
                        .map(str::trim)
                        .find(|c| !c.is_empty())
                        .map(str::to_string)
                }
                "URL" => input.source_url = Some(value.trim().to_string()),
                "DESCRIPTION" => {
                    let d: String = unescape(&value).chars().take(DESCRIPTION_MAX).collect();
                    input.attrs = Some(serde_json::json!({ "description": d }));
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok(rows)
}

// ───────────────────────────── Import ─────────────────────────────

/// Upserts parsed rows into `scope` (origin `import`, provenance
/// `import:<job>`). A row without `external_id` is refused: an import must
/// stay idempotent.
pub async fn run<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    scope: &Scope,
    rows: Vec<Row>,
) -> Result<RangeImportResult, sea_orm::DbErr> {
    let writer = Writer {
        origin: RangeOrigin::Import,
        source_ref: Some(format!("import:{}", uuid::Uuid::new_v4())),
    };
    let mut out = RangeImportResult::default();
    let reject = |out: &mut RangeImportResult, line: u32, field: &str, token: &str| {
        out.rejected += 1;
        if out.errors.len() < IMPORT_ERRORS_MAX {
            out.errors.push(RangeImportError {
                line,
                field: field.to_string(),
                token: token.to_string(),
            });
        }
    };
    for row in rows {
        if row
            .input
            .external_id
            .as_deref()
            .is_none_or(|e| e.trim().is_empty())
        {
            reject(&mut out, row.line, "external_id", err_codes::FIELD_REQUIRED);
            continue;
        }
        match upsert(db, org_id, scope, &row.input, &writer).await {
            Ok((_, true)) => out.created += 1,
            Ok((_, false)) => out.updated += 1,
            Err(RangeError::Invalid { field, token }) => reject(&mut out, row.line, &field, &token),
            Err(RangeError::Db(e)) => return Err(e),
            Err(e) => return Err(sea_orm::DbErr::Custom(e.to_string())),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_separators_and_lines() {
        let body = "\u{feff}external_id;label;planned_start;planned_end;extra\r\n\
                    a1;\"Le 7/9; matin\";2026-10-12T07:00:00+02:00;2026-10-12T09:00:00+02:00;x\r\n\
                    \r\n\
                    a2;\"Débat\n\"\"spécial\"\"\";;;\n";
        let rows = parse(Format::Csv, body.as_bytes()).expect("csv");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].line, 2);
        assert_eq!(rows[0].input.external_id.as_deref(), Some("a1"));
        assert_eq!(rows[0].input.label.as_deref(), Some("Le 7/9; matin"));
        assert_eq!(
            rows[0].input.planned_end.as_deref(),
            Some("2026-10-12T09:00:00+02:00")
        );
        assert_eq!(rows[1].line, 4);
        assert_eq!(rows[1].input.label.as_deref(), Some("Débat\n\"spécial\""));
        // Empty cells are absent fields.
        assert_eq!(rows[1].input.planned_start, None);
        assert_eq!(
            parse(Format::Csv, b"id,name\n1,x\n"),
            Err(ParseError::Invalid),
            "no label column"
        );
        assert_eq!(parse(Format::Csv, &[0xff, 0xfe]), Err(ParseError::Invalid));
        let many = format!("label\n{}", "x\n".repeat(IMPORT_MAX_ROWS + 1));
        assert_eq!(
            parse(Format::Csv, many.as_bytes()),
            Err(ParseError::TooManyRows)
        );
        let bad_conf = parse(Format::Csv, b"label,confidence\nx,abc\n").unwrap();
        assert!(bad_conf[0].input.confidence.is_some_and(f64::is_nan));
    }

    #[test]
    fn ics_events_times_and_folding() {
        let body = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:show-1@radio.example\r\n\
SUMMARY:Le journal\\, édition\r\n  du matin\r\n\
DTSTART;TZID=Europe/Paris:20261012T070000\r\n\
DTEND;TZID=Europe/Paris:20261012T073000\r\n\
CATEGORIES:news,info\r\n\
URL:https://radio.example/grille\r\n\
DESCRIPTION:Avec A.\\nEt B.\r\n\
BEGIN:VALARM\r\n\
DESCRIPTION:ignored\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:show-2\r\n\
SUMMARY:Nuit\r\n\
DTSTART:20261012T220000Z\r\n\
DURATION:PT1H30M\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:day\r\n\
SUMMARY:Journée\r\n\
DTSTART;VALUE=DATE:20261013\r\n\
DTEND;VALUE=DATE:20261014\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:bad\r\n\
DTSTART:demain\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
        let rows = parse(Format::Ics, body.as_bytes()).expect("ics");
        assert_eq!(rows.len(), 4);
        let a = &rows[0].input;
        assert_eq!(rows[0].line, 3);
        assert_eq!(a.external_id.as_deref(), Some("show-1@radio.example"));
        assert_eq!(a.label.as_deref(), Some("Le journal, édition du matin"));
        assert_eq!(
            a.planned_start.as_deref(),
            Some("2026-10-12T07:00:00+02:00")
        );
        assert_eq!(a.planned_end.as_deref(), Some("2026-10-12T07:30:00+02:00"));
        assert_eq!(a.category.as_deref(), Some("news"));
        assert_eq!(
            a.source_url.as_deref(),
            Some("https://radio.example/grille")
        );
        assert_eq!(
            a.attrs,
            Some(serde_json::json!({ "description": "Avec A.\nEt B." }))
        );
        let b = &rows[1].input;
        assert_eq!(
            b.planned_start.as_deref(),
            Some("2026-10-12T22:00:00+00:00")
        );
        assert_eq!(b.planned_end.as_deref(), Some("2026-10-12T23:30:00+00:00"));
        assert_eq!(
            rows[2].input.planned_start.as_deref(),
            Some("2026-10-13T00:00:00+00:00")
        );
        // Unreadable time kept raw (the shared check refuses it); no label.
        assert_eq!(rows[3].input.planned_start.as_deref(), Some("demain"));
        assert_eq!(rows[3].input.label.as_deref(), Some(""));
        assert_eq!(
            parse(Format::Ics, b"BEGIN:VEVENT\n"),
            Err(ParseError::Invalid)
        );
    }

    #[test]
    fn ics_durations() {
        assert_eq!(ics_duration("PT1H30M"), Some(5400));
        assert_eq!(ics_duration("P1DT1S"), Some(86_401));
        assert_eq!(ics_duration("P1W"), Some(604_800));
        assert_eq!(ics_duration("PT"), None);
        assert_eq!(ics_duration("P1H"), None);
        assert_eq!(ics_duration("1H"), None);
    }
}
