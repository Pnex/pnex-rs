//! ADS-B traces of adsb.lol read into a table (markets-research.md MK24):
//! where each part is, and the decoding of one trace into typed rows. AIS
//! and DVF are pipelines of their packs (pipeline-engine.md). Pure code: no I/O, no clock; the
//! backend streams the files and writes the rows (D807, D889).

use chrono::NaiveDate;

/// A source of large files, fixed in the code (D822, R9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BulkSource {
    pub key: &'static str,
    /// The table written, one per source (D807).
    pub table: &'static str,
    pub license_note: &'static str,
}

pub const ADSB_LOL: BulkSource = BulkSource {
    key: "adsb_lol",
    table: "adsb_lol_positions",
    license_note: "adsb.lol globe history: Open Database License (ODbL), cite adsb.lol \
                   and share a derived database under the same licence",
};

/// Part `index` (0 = `.aa`, 1 = `.ab`…) of a day of adsb.lol's history:
/// one tar split in parts of 2 GB.
pub fn adsb_part_url(day: NaiveDate, index: u8) -> Option<String> {
    if index >= 26 {
        return None;
    }
    let tag = format!("v{}-planes-readsb-prod-0", day.format("%Y.%m.%d"));
    Some(format!(
        "https://github.com/adsblol/globe_history_{}/releases/download/{tag}/{tag}.tar.a{}",
        day.format("%Y"),
        (b'a' + index) as char
    ))
}

/// A position on the globe, else `None` (AIS sends 91/181 for "not available").
fn place(lat: Option<f64>, lon: Option<f64>) -> Option<(f64, f64)> {
    let (lat, lon) = (lat?, lon?);
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then_some((lat, lon))
}

/// One aircraft position (ADS-B).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlightPoint {
    pub ts_us: i64,
    /// 24-bit ICAO address, hex.
    pub icao: String,
    pub registration: Option<String>,
    pub aircraft_type: Option<String>,
    pub lat: f64,
    pub lon: f64,
    /// Feet, barometric; `None` on the ground or unknown.
    pub alt_baro_ft: Option<f64>,
    pub on_ground: bool,
    /// Knots.
    pub ground_speed: Option<f64>,
    pub track: Option<f64>,
    /// Feet per minute.
    pub vertical_rate: Option<f64>,
    /// `adsb_icao`, `mlat`, `tisb_icao`…
    pub source: Option<String>,
}

/// A readsb `trace_full_<icao>.json`: a base `timestamp` and `trace` rows
/// `[seconds after, lat, lon, alt or "ground", gs, track, flags, rate,
/// details, source, …]`.
pub fn adsb_trace(json: &[u8]) -> Option<Vec<FlightPoint>> {
    let doc: serde_json::Value = serde_json::from_slice(json).ok()?;
    let icao = doc.get("icao")?.as_str()?.trim().to_ascii_lowercase();
    let base = doc.get("timestamp")?.as_f64()?;
    let s = |k: &str| {
        doc.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let (registration, aircraft_type) = (s("r"), s("t"));
    let mut out = Vec::new();
    for row in doc.get("trace")?.as_array()? {
        let Some(r) = row.as_array() else { continue };
        let n = |i: usize| r.get(i).and_then(|v| v.as_f64()).filter(|x| x.is_finite());
        let (Some(dt), Some((lat, lon))) = (n(0), place(n(1), n(2))) else {
            continue;
        };
        let on_ground = r.get(3).and_then(|v| v.as_str()) == Some("ground");
        out.push(FlightPoint {
            ts_us: ((base + dt) * 1e6).round() as i64,
            icao: icao.clone(),
            registration: registration.clone(),
            aircraft_type: aircraft_type.clone(),
            lat,
            lon,
            alt_baro_ft: if on_ground { None } else { n(3) },
            on_ground,
            ground_speed: n(4),
            track: n(5),
            vertical_rate: n(7),
            source: r.get(9).and_then(|v| v.as_str()).map(str::to_string),
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adsb_parts_are_fixed_per_day() {
        let d = NaiveDate::from_ymd_opt(2025, 12, 30).unwrap();
        assert_eq!(
            adsb_part_url(d, 1).unwrap(),
            "https://github.com/adsblol/globe_history_2025/releases/download/\
             v2025.12.30-planes-readsb-prod-0/v2025.12.30-planes-readsb-prod-0.tar.ab"
        );
        assert!(adsb_part_url(d, 26).is_none());
    }

    #[test]
    fn an_adsb_trace_gives_dated_points() {
        let j = br#"{"icao":"AB32AB","r":"N820UP","t":"B350","timestamp": 1767052800.000,
            "trace":[[72946.01,31.209225,-81.366724,3775,144.2,356.8,5,3008,null,"adsb_icao",3800],
                     [72950.0,31.21,-81.36,"ground",3.0,10.0,0,null,null,"mlat"],
                     [72951.0,null,null,100]]}"#;
        let pts = adsb_trace(j).unwrap();
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].icao, "ab32ab");
        assert_eq!(pts[0].ts_us, 1_767_125_746_010_000);
        assert_eq!(pts[0].alt_baro_ft, Some(3775.0));
        assert_eq!(pts[0].source.as_deref(), Some("adsb_icao"));
        assert!(pts[1].on_ground && pts[1].alt_baro_ft.is_none());
        assert!(adsb_trace(b"{}").is_none());
    }
}
