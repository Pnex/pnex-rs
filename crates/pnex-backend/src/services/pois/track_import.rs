//! Track files (cases-workspace.md K26): GPX (track points) and CSV (a time
//! column, latitude, longitude, optional altitude) read into fixes. Times
//! with an offset or a `Z` are exact; times without one need the time zone
//! of the recording, never guessed. Every unreadable row is reported, not
//! dropped silently.

use chrono::{DateTime, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use pnex_core::track::{RejectReason, Rejected, TrackPoint};
use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Gpx,
    Csv,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "gpx" => Some(Self::Gpx),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum When {
    Utc(i64),
    Local(NaiveDateTime),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Fix {
    row: usize,
    when: When,
    lon: f64,
    lat: f64,
    alt: Option<f64>,
}

/// A read file: its fixes, and the rows that could not be read.
#[derive(Debug, Default)]
pub struct Parsed {
    fixes: Vec<Fix>,
    pub unreadable: Vec<usize>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ImportError {
    /// Not a GPX document / no usable CSV header.
    Unreadable,
    /// Times without offset and no time zone given.
    TimezoneRequired,
}

impl Parsed {
    /// Whether some times carry no offset (a time zone is then required).
    pub fn needs_time_zone(&self) -> bool {
        self.fixes.iter().any(|f| matches!(f.when, When::Local(_)))
    }

    /// Points in UTC; local times are placed in `tz`. A local time that does
    /// not exist (spring forward) is rejected as unreadable; an ambiguous one
    /// (fall back) takes the earlier instant.
    pub fn resolve(self, tz: Option<Tz>) -> Result<(Vec<TrackPoint>, Vec<Rejected>), ImportError> {
        if self.needs_time_zone() && tz.is_none() {
            return Err(ImportError::TimezoneRequired);
        }
        let mut rejected: Vec<Rejected> = self
            .unreadable
            .iter()
            .map(|&index| Rejected {
                index,
                reason: RejectReason::Unreadable,
            })
            .collect();
        let mut points = Vec::with_capacity(self.fixes.len());
        for f in self.fixes {
            let t_ms = match (f.when, tz) {
                (When::Utc(ms), _) => Some(ms),
                (When::Local(n), Some(tz)) => tz
                    .from_local_datetime(&n)
                    .earliest()
                    .map(|t| t.timestamp_millis()),
                (When::Local(_), None) => None,
            };
            match t_ms {
                Some(t_ms) => points.push((
                    f.row,
                    TrackPoint {
                        t_ms,
                        lon: f.lon,
                        lat: f.lat,
                        alt: f.alt,
                    },
                )),
                None => rejected.push(Rejected {
                    index: f.row,
                    reason: RejectReason::Unreadable,
                }),
            }
        }
        // Normalisation reports by position in its input: map back to rows.
        let rows: Vec<usize> = points.iter().map(|(r, _)| *r).collect();
        let n = pnex_core::track::normalize(points.into_iter().map(|(_, p)| p).collect());
        rejected.extend(n.rejected.into_iter().map(|r| Rejected {
            index: rows[r.index],
            reason: r.reason,
        }));
        rejected.sort_by_key(|r| r.index);
        Ok((n.points, rejected))
    }
}

fn when(raw: &str) -> Option<When> {
    let s = raw.trim().trim_matches('"');
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(When::Utc(t.timestamp_millis()));
    }
    if let Ok(n) = s.parse::<i64>() {
        // Epoch: milliseconds past ~1973, else seconds.
        return Some(When::Utc(if n.abs() > 100_000_000_000 {
            n
        } else {
            n * 1_000
        }));
    }
    for f in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, f) {
            return Some(When::Local(n));
        }
    }
    None
}

fn number(raw: &str, decimal_comma: bool) -> Option<f64> {
    let s = raw.trim().trim_matches('"');
    let s = if decimal_comma {
        s.replace(',', ".")
    } else {
        s.to_string()
    };
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Reads a file. `Err` only when the document as a whole is unusable.
pub fn parse(format: Format, text: &str) -> Result<Parsed, ImportError> {
    match format {
        Format::Gpx => gpx(text),
        Format::Csv => csv(text),
    }
}

/// `lon` and `lat` attributes of a track point, as the start of its reading.
fn lon_lat(
    e: &quick_xml::events::BytesStart,
) -> (Option<f64>, Option<f64>, Option<f64>, Option<When>) {
    let mut p = (None, None, None, None);
    for a in e.attributes().flatten() {
        let v = std::str::from_utf8(&a.value)
            .ok()
            .and_then(|v| number(v, false));
        match a.key.local_name().as_ref() {
            b"lon" => p.0 = v,
            b"lat" => p.1 = v,
            _ => {}
        }
    }
    p
}

fn gpx(text: &str) -> Result<Parsed, ImportError> {
    let mut reader = Reader::from_str(text);
    let mut out = Parsed::default();
    let mut seen_gpx = false;
    let mut row = 0usize;
    // The point being read: lon, lat, ele, time.
    let mut cur: Option<(Option<f64>, Option<f64>, Option<f64>, Option<When>)> = None;
    let mut field: Option<&'static str> = None;
    let finish = |row: usize,
                  cur: (Option<f64>, Option<f64>, Option<f64>, Option<When>),
                  out: &mut Parsed| match cur {
        (Some(lon), Some(lat), alt, Some(when)) => out.fixes.push(Fix {
            row,
            when,
            lon,
            lat,
            alt,
        }),
        _ => out.unreadable.push(row),
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"trkpt" => {
                row += 1;
                cur = Some(lon_lat(&e));
            }
            // A self-closing point has no time: unreadable.
            Ok(Event::Empty(e)) if e.local_name().as_ref() == b"trkpt" => {
                row += 1;
                out.unreadable.push(row);
            }
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"gpx" => seen_gpx = true,
                b"ele" if cur.is_some() => field = Some("ele"),
                b"time" if cur.is_some() => field = Some("time"),
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if let (Some(f), Some(p)) = (field, cur.as_mut()) {
                    let s = String::from_utf8_lossy(&t).to_string();
                    match f {
                        "ele" => p.2 = number(&s, false),
                        _ => p.3 = when(&s),
                    }
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"trkpt" => {
                    if let Some(p) = cur.take() {
                        finish(row, p, &mut out);
                    }
                }
                b"ele" | b"time" => field = None,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => return Err(ImportError::Unreadable),
            _ => {}
        }
    }
    if !seen_gpx {
        return Err(ImportError::Unreadable);
    }
    Ok(out)
}

fn csv(text: &str) -> Result<Parsed, ImportError> {
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty());
    let (_, header) = lines.next().ok_or(ImportError::Unreadable)?;
    let delim = [';', '\t', ',']
        .into_iter()
        .max_by_key(|d| header.matches(*d).count())
        .unwrap_or(',');
    let cols: Vec<String> = header
        .split(delim)
        .map(|c| c.trim().trim_matches('"').to_lowercase())
        .collect();
    let find = |names: &[&str]| cols.iter().position(|c| names.contains(&c.as_str()));
    let t = find(&["time", "timestamp", "datetime", "date_time", "date", "utc"]);
    let lat = find(&["lat", "latitude"]);
    let lon = find(&["lon", "lng", "long", "longitude"]);
    let alt = find(&["alt", "altitude", "ele", "elevation"]);
    let (Some(t), Some(lat), Some(lon)) = (t, lat, lon) else {
        return Err(ImportError::Unreadable);
    };
    let decimal_comma = delim == ';';
    let mut out = Parsed::default();
    for (i, line) in lines {
        let row = i + 1;
        let cells: Vec<&str> = line.split(delim).collect();
        let get = |k: usize| cells.get(k).copied();
        match (
            get(t).and_then(when),
            get(lon).and_then(|v| number(v, decimal_comma)),
            get(lat).and_then(|v| number(v, decimal_comma)),
        ) {
            (Some(when), Some(lon), Some(lat)) => out.fixes.push(Fix {
                row,
                when,
                lon,
                lat,
                alt: alt.and_then(get).and_then(|v| number(v, decimal_comma)),
            }),
            _ => out.unreadable.push(row),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPX: &str = r#"<?xml version="1.0"?>
<gpx version="1.1" xmlns="http://www.topografix.com/GPX/1/1"><trk><trkseg>
  <trkpt lat="48.0" lon="2.0"><ele>35.5</ele><time>2026-03-01T10:00:00Z</time></trkpt>
  <trkpt lat="48.001" lon="2.001"><time>2026-03-01T12:00:01+02:00</time></trkpt>
  <trkpt lat="48.002" lon="2.002"></trkpt>
  <trkpt lat="91" lon="2.0"><time>2026-03-01T10:00:02Z</time></trkpt>
</trkseg></trk></gpx>"#;

    #[test]
    fn gpx_reads_points_and_reports_bad_ones() {
        let p = parse(Format::Gpx, GPX).unwrap();
        assert!(!p.needs_time_zone());
        let (points, rejected) = p.resolve(None).unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].alt, Some(35.5));
        assert_eq!(points[1].t_ms - points[0].t_ms, 1_000, "offset applied");
        assert_eq!(
            rejected,
            vec![
                Rejected {
                    index: 3,
                    reason: RejectReason::Unreadable
                },
                Rejected {
                    index: 4,
                    reason: RejectReason::OutOfRange
                },
            ]
        );
        assert_eq!(
            parse(Format::Gpx, "<kml/>").unwrap_err(),
            ImportError::Unreadable
        );
    }

    #[test]
    fn csv_needs_a_time_zone_for_local_times() {
        let text = "Date;Latitude;Longitude;Altitude\n2026-03-01 10:00:00;48,0;2,0;35\n2026-03-01 10:00:01;48,001;2,001;\nbad;48;2;\n";
        let p = parse(Format::Csv, text).unwrap();
        assert!(p.needs_time_zone());
        assert_eq!(p.unreadable, vec![4]);
        let p = parse(Format::Csv, text).unwrap();
        assert_eq!(p.resolve(None).unwrap_err(), ImportError::TimezoneRequired);
        let p = parse(Format::Csv, text).unwrap();
        let (points, rejected) = p.resolve(Some(chrono_tz::Europe::Paris)).unwrap();
        // 10:00 in Paris in March (CET) is 09:00 UTC.
        assert_eq!(points[0].t_ms, 1_772_355_600_000);
        assert_eq!((points[0].lat, points[0].alt), (48.0, Some(35.0)));
        assert_eq!(rejected.len(), 1);
    }

    #[test]
    fn csv_reads_epochs_and_needs_its_columns() {
        let p = parse(
            Format::Csv,
            "timestamp,lat,lon\n1772355600,48.0,2.0\n1772355601000,48.1,2.1\n",
        )
        .unwrap();
        let (points, _) = p.resolve(None).unwrap();
        assert_eq!(points[1].t_ms - points[0].t_ms, 1_000);
        assert_eq!(
            parse(Format::Csv, "a,b\n1,2\n").unwrap_err(),
            ImportError::Unreadable
        );
    }
}
