//! Large public files read into tables (markets-research.md appendix F:
//! MK22 DVF, MK23 AIS, MK24 ADS-B): where each file is, and the decoding of
//! one line or one trace into typed rows. Pure code: no I/O, no clock; the
//! backend streams the files and writes the rows (D807, D889).

use chrono::{NaiveDate, NaiveDateTime};

/// A source of large files, fixed in the code (D822, R9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BulkSource {
    pub key: &'static str,
    /// The table written, one per source (D807).
    pub table: &'static str,
    pub license_note: &'static str,
}

pub const AIS_DK: BulkSource = BulkSource {
    key: "ais_dk",
    table: "ais_dk_positions",
    license_note: "Danish Maritime Authority AIS data: terms of the authority, \
                   to check before any use outside the org",
};

pub const AIS_NOAA: BulkSource = BulkSource {
    key: "ais_noaa",
    table: "ais_noaa_positions",
    license_note: "NOAA Office for Coastal Management and BOEM (MarineCadastre): \
                   US public domain, cite the source",
};

pub const ADSB_LOL: BulkSource = BulkSource {
    key: "adsb_lol",
    table: "adsb_lol_positions",
    license_note: "adsb.lol globe history: Open Database License (ODbL), cite adsb.lol \
                   and share a derived database under the same licence",
};

pub const DVF: BulkSource = BulkSource {
    key: "dvf",
    table: "dvf_mutations",
    license_note: "DVF (Demandes de valeurs foncières, DGFiP, geolocated by Etalab): \
                   Licence Ouverte 2.0; never used to re-identify a person",
};

/// The Danish file of a day (the authority serves it over plain HTTP only).
pub fn ais_dk_url(day: NaiveDate) -> String {
    format!("http://aisdata.ais.dk/aisdk-{}.zip", day.format("%Y-%m-%d"))
}

pub fn ais_noaa_url(day: NaiveDate) -> String {
    format!(
        "https://coast.noaa.gov/htdata/CMSP/AISDataHandler/{}/AIS_{}.zip",
        day.format("%Y"),
        day.format("%Y_%m_%d")
    )
}

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

/// The whole of France for a year, gzipped CSV (about 100 MB).
pub fn dvf_url(year: i32) -> String {
    format!("https://files.data.gouv.fr/geo-dvf/latest/csv/{year}/full.csv.gz")
}

/// One CSV line in fields: `,` separated, `"` quoted (a quoted field may
/// hold a comma and `""`).
pub fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.trim_end_matches(['\r', '\n']).chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// One vessel position (AIS).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Position {
    /// µs since the epoch, UTC.
    pub ts_us: i64,
    pub mmsi: i64,
    pub lat: f64,
    pub lon: f64,
    /// Knots.
    pub sog: Option<f64>,
    pub cog: Option<f64>,
    pub heading: Option<f64>,
    pub nav_status: Option<String>,
    pub name: Option<String>,
    pub imo: Option<String>,
    pub callsign: Option<String>,
    pub ship_type: Option<String>,
    /// Metres.
    pub length: Option<f64>,
    pub width: Option<f64>,
    pub draught: Option<f64>,
    pub destination: Option<String>,
}

fn text(v: Option<&String>) -> Option<String> {
    v.map(|s| s.trim())
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("unknown") && *s != "Undefined")
        .map(str::to_string)
}

fn num(v: Option<&String>) -> Option<f64> {
    v.and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|x| x.is_finite())
}

/// A position on the globe, else `None` (AIS sends 91/181 for "not available").
fn place(lat: Option<f64>, lon: Option<f64>) -> Option<(f64, f64)> {
    let (lat, lon) = (lat?, lon?);
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then_some((lat, lon))
}

/// The Danish header, checked once per file.
pub const AIS_DK_HEADER: &str = "# Timestamp,Type of mobile,MMSI,Latitude,Longitude";

/// A line of the Danish file: `dd/mm/yyyy HH:MM:SS` UTC, then the fields of
/// [`AIS_DK_HEADER`] and on. Base stations and aids to navigation are left
/// out: only vessels (`Class A`, `Class B`).
pub fn ais_dk_line(line: &str) -> Option<Position> {
    let f = split_csv(line);
    if !matches!(f.get(1)?.as_str(), "Class A" | "Class B") {
        return None;
    }
    let ts = NaiveDateTime::parse_from_str(f.first()?, "%d/%m/%Y %H:%M:%S").ok()?;
    let (lat, lon) = place(num(f.get(3)), num(f.get(4)))?;
    Some(Position {
        ts_us: ts.and_utc().timestamp_micros(),
        mmsi: f.get(2)?.trim().parse().ok()?,
        lat,
        lon,
        nav_status: text(f.get(5)),
        sog: num(f.get(7)),
        cog: num(f.get(8)),
        heading: num(f.get(9)).filter(|h| *h < 360.0),
        imo: text(f.get(10)),
        callsign: text(f.get(11)),
        name: text(f.get(12)),
        ship_type: text(f.get(13)),
        width: num(f.get(15)),
        length: num(f.get(16)),
        draught: num(f.get(18)),
        destination: text(f.get(19)),
    })
}

pub const AIS_NOAA_HEADER: &str = "MMSI,BaseDateTime,LAT,LON";

/// A line of the NOAA file: `MMSI,BaseDateTime,LAT,LON,SOG,COG,Heading,
/// VesselName,IMO,CallSign,VesselType,Status,Length,Width,Draft,…`, UTC.
pub fn ais_noaa_line(line: &str) -> Option<Position> {
    let f = split_csv(line);
    let ts = NaiveDateTime::parse_from_str(f.get(1)?, "%Y-%m-%dT%H:%M:%S").ok()?;
    let (lat, lon) = place(num(f.get(2)), num(f.get(3)))?;
    Some(Position {
        ts_us: ts.and_utc().timestamp_micros(),
        mmsi: f.first()?.trim().parse().ok()?,
        lat,
        lon,
        sog: num(f.get(4)),
        cog: num(f.get(5)),
        // 511 = not available.
        heading: num(f.get(6)).filter(|h| *h < 360.0),
        name: text(f.get(7)),
        imo: text(f.get(8)),
        callsign: text(f.get(9)),
        ship_type: text(f.get(10)),
        nav_status: text(f.get(11)),
        length: num(f.get(12)),
        width: num(f.get(13)),
        draught: num(f.get(14)),
        destination: None,
    })
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

/// One line of a DVF file: a property of a sale (a sale of several lots or
/// parcels has several lines with the same `mutation_id`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mutation {
    pub mutation_id: String,
    pub date: NaiveDate,
    /// `Vente`, `Echange`, `Adjudication`…
    pub nature: Option<String>,
    /// Euros, for the whole sale.
    pub value_eur: Option<f64>,
    pub postal_code: Option<String>,
    pub commune_code: Option<String>,
    pub commune_name: Option<String>,
    pub department_code: Option<String>,
    pub parcel_id: Option<String>,
    /// `Appartement`, `Maison`, `Dépendance`, `Local industriel…`.
    pub premises_type: Option<String>,
    pub built_area_m2: Option<f64>,
    pub rooms: Option<f64>,
    pub land_area_m2: Option<f64>,
    pub lon: Option<f64>,
    pub lat: Option<f64>,
}

/// Positions of the columns read, from the header line (`None` when it is
/// not a DVF header).
#[derive(Debug, Clone, PartialEq)]
pub struct DvfColumns([usize; 15]);

pub fn dvf_columns(header: &str) -> Option<DvfColumns> {
    const NAMES: [&str; 15] = [
        "id_mutation",
        "date_mutation",
        "nature_mutation",
        "valeur_fonciere",
        "code_postal",
        "code_commune",
        "nom_commune",
        "code_departement",
        "id_parcelle",
        "type_local",
        "surface_reelle_bati",
        "nombre_pieces_principales",
        "surface_terrain",
        "longitude",
        "latitude",
    ];
    let h = split_csv(header);
    let mut at = [0usize; 15];
    for (i, n) in NAMES.iter().enumerate() {
        at[i] = h.iter().position(|c| c.trim() == *n)?;
    }
    Some(DvfColumns(at))
}

pub fn dvf_line(cols: &DvfColumns, line: &str) -> Option<Mutation> {
    let f = split_csv(line);
    let g = |i: usize| f.get(cols.0[i]);
    let raw = |i: usize| {
        g(i).map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(Mutation {
        mutation_id: raw(0)?,
        date: NaiveDate::parse_from_str(g(1)?.trim(), "%Y-%m-%d").ok()?,
        nature: raw(2),
        value_eur: num(g(3)),
        postal_code: raw(4),
        commune_code: raw(5),
        commune_name: raw(6),
        department_code: raw(7),
        parcel_id: raw(8),
        premises_type: raw(9),
        built_area_m2: num(g(10)),
        rooms: num(g(11)),
        land_area_m2: num(g(12)),
        lon: num(g(13)),
        lat: num(g(14)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_fixed_per_day_and_year() {
        let d = NaiveDate::from_ymd_opt(2025, 12, 30).unwrap();
        assert_eq!(ais_dk_url(d), "http://aisdata.ais.dk/aisdk-2025-12-30.zip");
        assert_eq!(
            ais_noaa_url(d),
            "https://coast.noaa.gov/htdata/CMSP/AISDataHandler/2025/AIS_2025_12_30.zip"
        );
        assert_eq!(
            adsb_part_url(d, 1).unwrap(),
            "https://github.com/adsblol/globe_history_2025/releases/download/\
             v2025.12.30-planes-readsb-prod-0/v2025.12.30-planes-readsb-prod-0.tar.ab"
        );
        assert!(adsb_part_url(d, 26).is_none());
        assert_eq!(
            dvf_url(2024),
            "https://files.data.gouv.fr/geo-dvf/latest/csv/2024/full.csv.gz"
        );
    }

    #[test]
    fn csv_fields_keep_quoted_commas() {
        assert_eq!(
            split_csv("a,\"b, c\",\"d\"\"e\",\r\n"),
            ["a", "b, c", "d\"e", ""]
        );
    }

    #[test]
    fn danish_lines_keep_vessels_only() {
        let base = "01/11/2026 00:00:00,Base Station,2190064,56.716647,11.518977,Unknown value,,,,,Unknown,Unknown,,Undefined,,,,GPS,,Unknown,,AIS,,,,";
        assert!(ais_dk_line(base).is_none());
        let ship = "01/11/2026 00:00:01,Class A,305287000,57.402440,8.739752,Under way using engine,0.0,15.7,225.0,222,IMO9395707,OZ123,NORD STAR,Cargo,,32,190,GPS,9.5,ROTTERDAM,,AIS,,,,";
        let p = ais_dk_line(ship).unwrap();
        assert_eq!(p.mmsi, 305287000);
        assert_eq!(p.ts_us, 1_793_491_201_000_000);
        assert_eq!(p.sog, Some(15.7));
        assert_eq!(p.name.as_deref(), Some("NORD STAR"));
        assert_eq!(
            (p.width, p.length, p.draught),
            (Some(32.0), Some(190.0), Some(9.5))
        );
        assert_eq!(p.destination.as_deref(), Some("ROTTERDAM"));
        let nowhere = "01/11/2026 00:00:01,Class B,1,91,181,,,,,,,,,,,,,,,,,AIS,,,,";
        assert!(ais_dk_line(nowhere).is_none(), "91/181 is no position");
    }

    #[test]
    fn noaa_lines_read_the_us_columns() {
        let l = "367776660,2024-12-31T00:00:08,21.19308,-157.72342,8.0,112.1,511.0,MOUNT BAKER,IMO9822906,WDJ6673,52,0,40,12,5.5,52,A";
        let p = ais_noaa_line(l).unwrap();
        assert_eq!(p.mmsi, 367776660);
        assert_eq!(p.heading, None, "511 is not available");
        assert_eq!(p.ship_type.as_deref(), Some("52"));
        assert_eq!(p.length, Some(40.0));
        assert!(ais_noaa_line(AIS_NOAA_HEADER).is_none());
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

    #[test]
    fn dvf_lines_are_read_by_header_names() {
        let h = "id_mutation,date_mutation,numero_disposition,nature_mutation,valeur_fonciere,adresse_numero,adresse_suffixe,adresse_nom_voie,adresse_code_voie,code_postal,code_commune,nom_commune,code_departement,ancien_code_commune,ancien_nom_commune,id_parcelle,ancien_id_parcelle,numero_volume,lot1_numero,lot1_surface_carrez,lot2_numero,lot2_surface_carrez,lot3_numero,lot3_surface_carrez,lot4_numero,lot4_surface_carrez,lot5_numero,lot5_surface_carrez,nombre_lots,code_type_local,type_local,surface_reelle_bati,nombre_pieces_principales,code_nature_culture,nature_culture,code_nature_culture_speciale,nature_culture_speciale,surface_terrain,longitude,latitude";
        let cols = dvf_columns(h).unwrap();
        let l = "2025-1273242,2025-01-03,000001,Vente,430500,88,,RUE MARCADET,5995,75018,75118,Paris 18e Arrondissement,75,,,75118000BJ0092,,,226,49.48,227,12,228,49.48,237,,,,4,2,Appartement,49,2,,,,,,2.346344,48.890689";
        let m = dvf_line(&cols, l).unwrap();
        assert_eq!(m.mutation_id, "2025-1273242");
        assert_eq!(m.value_eur, Some(430500.0));
        assert_eq!(m.premises_type.as_deref(), Some("Appartement"));
        assert_eq!((m.built_area_m2, m.rooms), (Some(49.0), Some(2.0)));
        assert_eq!(m.commune_code.as_deref(), Some("75118"));
        assert_eq!(m.lat, Some(48.890689));
        assert!(dvf_columns("a,b").is_none());
        assert!(dvf_line(&cols, "x,soon").is_none());
    }
}
