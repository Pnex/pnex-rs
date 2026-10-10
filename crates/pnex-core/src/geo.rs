//! Geo providers contract (`docs/architecture/geo-layers.md` §8, phase F):
//! capabilities, adapter kinds, normalized DTOs (L18) and the pure
//! request/response mapping of each native adapter (L17). Wasm-safe: the
//! HTTP call, the cache and the rate limit live in the backend proxy (L20).
//!
//! The UI, imports and flows only ever see [`GeocodeResult`] and [`Route`];
//! switching engines never leaks a provider format.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

/// What a provider can do (L16). One default provider per capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeoCapability {
    Basemap,
    Geocode,
    Reverse,
    Autocomplete,
    Route,
    Isochrone,
    Matrix,
}

/// Native adapter (L17). Phase F ships Nominatim / Photon (geocoding) and
/// Valhalla / GraphHopper (routing).
/// Every adapter authenticates with one optional API key sent as a query
/// parameter, the way map providers do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeoProviderKind {
    /// MapLibre style / tile template / PMTiles archive (L24).
    Basemap,
    Nominatim,
    Photon,
    Valhalla,
    GraphHopper,
}

impl GeoProviderKind {
    pub const ALL: [GeoProviderKind; 5] = [
        GeoProviderKind::Basemap,
        GeoProviderKind::Nominatim,
        GeoProviderKind::Photon,
        GeoProviderKind::Valhalla,
        GeoProviderKind::GraphHopper,
    ];

    /// Capabilities the adapter is able to serve; a provider row may enable
    /// a subset, never more.
    pub fn supported(self) -> &'static [GeoCapability] {
        use GeoCapability::*;
        match self {
            GeoProviderKind::Basemap => &[Basemap],
            GeoProviderKind::Nominatim | GeoProviderKind::Photon => &[Geocode, Reverse],
            GeoProviderKind::Valhalla => &[Route, Isochrone, Matrix],
            GeoProviderKind::GraphHopper => &[Route],
        }
    }
}

impl GeoCapability {
    pub const ALL: [GeoCapability; 7] = [
        GeoCapability::Basemap,
        GeoCapability::Geocode,
        GeoCapability::Reverse,
        GeoCapability::Autocomplete,
        GeoCapability::Route,
        GeoCapability::Isochrone,
        GeoCapability::Matrix,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            GeoCapability::Basemap => "basemap",
            GeoCapability::Geocode => "geocode",
            GeoCapability::Reverse => "reverse",
            GeoCapability::Autocomplete => "autocomplete",
            GeoCapability::Route => "route",
            GeoCapability::Isochrone => "isochrone",
            GeoCapability::Matrix => "matrix",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

impl GeoProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GeoProviderKind::Basemap => "basemap",
            GeoProviderKind::Nominatim => "nominatim",
            GeoProviderKind::Photon => "photon",
            GeoProviderKind::Valhalla => "valhalla",
            GeoProviderKind::GraphHopper => "graphhopper",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// Query parameter carrying the API key when the provider sets none
/// (Google, MapTiler, LocationIQ use `key`).
pub const DEFAULT_KEY_PARAM: &str = "key";

/// Read model of a provider (`GET /api/v1/geo/providers`). The secret is a
/// vault reference, never a value (R4, R16).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeoProvider {
    pub id: uuid::Uuid,
    pub name: String,
    pub kind: GeoProviderKind,
    pub capabilities: Vec<GeoCapability>,
    pub base_url: String,
    /// Vault reference of the API key, if any.
    #[serde(default)]
    pub api_key: Option<crate::SecretFieldView>,
    /// Query parameter the key is sent in (`key`, `access_token`, …).
    pub key_param: String,
    #[serde(default)]
    pub params: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub rate_limit_per_s: Option<f32>,
    pub timeout_ms: u32,
    pub store_allowed: bool,
    /// Capabilities this provider is the org default for.
    #[serde(default)]
    pub default_for: Vec<GeoCapability>,
    pub updated_at: String,
}

/// Create / update body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoProviderInput {
    pub name: String,
    pub kind: GeoProviderKind,
    pub capabilities: Vec<GeoCapability>,
    pub base_url: String,
    /// API key; `None` keeps the current one on update.
    #[serde(default)]
    pub api_key: Option<crate::SecretFieldInput>,
    /// Query parameter of the key; `None` = [`DEFAULT_KEY_PARAM`].
    #[serde(default)]
    pub key_param: Option<String>,
    #[serde(default)]
    pub params: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub rate_limit_per_s: Option<f32>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u32,
    #[serde(default)]
    pub store_allowed: bool,
    /// Capabilities to make this provider the org default for.
    #[serde(default)]
    pub default_for: Vec<GeoCapability>,
}

fn default_timeout_ms() -> u32 {
    5_000
}

/// Basemap the map opens on (`GET /api/v1/geo/basemaps`). A basemap key
/// is a browser key (like a Google Maps key, restricted by referrer at the
/// provider): it is already in `style_url`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Basemap {
    pub id: uuid::Uuid,
    pub name: String,
    /// MapLibre style URL (spec v8).
    pub style_url: String,
    #[serde(default)]
    pub style_url_dark: Option<String>,
    pub is_default: bool,
}

/// Normalized geocoding hit (L18).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeocodeResult {
    pub label: String,
    pub lat: f64,
    pub lon: f64,
    /// `[west, south, east, north]` when the provider gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
    /// Provider place type (`house`, `city`, …), free text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 0..=1 when the provider ranks its hits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Address parts as given (`road`, `city`, `postcode`, …).
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub address: serde_json::Map<String, Value>,
}

/// One maneuver of a [`Route`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteStep {
    pub instruction: String,
    pub distance_m: f64,
    pub duration_s: f64,
    /// Index into the route geometry where the step starts.
    pub start_index: usize,
}

/// Normalized route (L18): geometry already decoded to GeoJSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Route {
    /// GeoJSON `LineString` (`[lon, lat]` pairs).
    pub geometry: Value,
    pub distance_m: f64,
    pub duration_s: f64,
    pub steps: Vec<RouteStep>,
}

/// Mapping failure of a provider response or request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeoError {
    /// Base URL unusable (not http(s), cannot be a base).
    BadBaseUrl,
    /// The provider answered with its own error payload.
    Provider(String),
    /// The response does not have the expected shape.
    Malformed(&'static str),
}

/// Joins `path` under the provider base URL, keeping any base path prefix
/// (`https://host/nominatim` + `search` → `https://host/nominatim/search`).
fn endpoint(base: &str, path: &str) -> Result<Url, GeoError> {
    let mut url = Url::parse(base).map_err(|_| GeoError::BadBaseUrl)?;
    if !matches!(url.scheme(), "http" | "https") || url.cannot_be_a_base() {
        return Err(GeoError::BadBaseUrl);
    }
    url.path_segments_mut()
        .map_err(|_| GeoError::BadBaseUrl)?
        .pop_if_empty()
        .push(path);
    Ok(url)
}

// ---------------------------------------------------------------- Nominatim

/// `GET {base}/search` URL. `params` are the provider's non-secret extra
/// query parameters (`accept-language`, `countrycodes`, …).
pub fn nominatim_search_url(
    base: &str,
    query: &str,
    limit: u32,
    params: &[(String, String)],
) -> Result<Url, GeoError> {
    let mut url = endpoint(base, "search")?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("format", "jsonv2")
        .append_pair("addressdetails", "1")
        .append_pair("limit", &limit.clamp(1, 50).to_string())
        .extend_pairs(params);
    Ok(url)
}

/// `GET {base}/reverse` URL.
pub fn nominatim_reverse_url(
    base: &str,
    lat: f64,
    lon: f64,
    params: &[(String, String)],
) -> Result<Url, GeoError> {
    let mut url = endpoint(base, "reverse")?;
    url.query_pairs_mut()
        .append_pair("lat", &lat.to_string())
        .append_pair("lon", &lon.to_string())
        .append_pair("format", "jsonv2")
        .append_pair("addressdetails", "1")
        .extend_pairs(params);
    Ok(url)
}

/// Nominatim sends coordinates as strings; tolerate numbers too.
fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn nominatim_place(p: &Value) -> Option<GeocodeResult> {
    // boundingbox = [south, north, west, east].
    let bbox = p["boundingbox"].as_array().and_then(|b| {
        let v: Vec<f64> = b.iter().filter_map(num).collect();
        (v.len() == 4).then(|| [v[2], v[0], v[3], v[1]])
    });
    Some(GeocodeResult {
        label: p["display_name"].as_str()?.to_owned(),
        lat: num(&p["lat"])?,
        lon: num(&p["lon"])?,
        bbox,
        kind: p["type"].as_str().map(str::to_owned),
        confidence: num(&p["importance"]).map(|i| i.clamp(0.0, 1.0)),
        address: p["address"].as_object().cloned().unwrap_or_default(),
    })
}

/// `/search` body (array) or `/reverse` body (one object) → hits. A reverse
/// miss (`{"error": "Unable to geocode"}`) is an empty list, not an error.
pub fn parse_nominatim(body: &Value) -> Result<Vec<GeocodeResult>, GeoError> {
    match body {
        Value::Array(places) => Ok(places.iter().filter_map(nominatim_place).collect()),
        Value::Object(o) if o.contains_key("error") => Ok(Vec::new()),
        Value::Object(_) => Ok(nominatim_place(body).into_iter().collect()),
        _ => Err(GeoError::Malformed("nominatim body")),
    }
}

// ------------------------------------------------------------------- Photon

/// `GET {base}/api` URL (forward geocoding).
pub fn photon_search_url(
    base: &str,
    query: &str,
    limit: u32,
    params: &[(String, String)],
) -> Result<Url, GeoError> {
    let mut url = endpoint(base, "api")?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("limit", &limit.clamp(1, 50).to_string())
        .extend_pairs(params);
    Ok(url)
}

/// `GET {base}/reverse` URL.
pub fn photon_reverse_url(
    base: &str,
    lat: f64,
    lon: f64,
    params: &[(String, String)],
) -> Result<Url, GeoError> {
    let mut url = endpoint(base, "reverse")?;
    url.query_pairs_mut()
        .append_pair("lat", &lat.to_string())
        .append_pair("lon", &lon.to_string())
        .extend_pairs(params);
    Ok(url)
}

/// Readable label of a Photon feature: name, street and number, postcode
/// and city, country (parts that exist, without repeats).
fn photon_label(p: &Value) -> String {
    let get = |k: &str| p[k].as_str().filter(|v| !v.is_empty());
    let street = match (get("housenumber"), get("street")) {
        (Some(n), Some(s)) => Some(format!("{n} {s}")),
        (None, Some(s)) => Some(s.to_owned()),
        _ => None,
    };
    let city = match (get("postcode"), get("city")) {
        (Some(pc), Some(c)) => Some(format!("{pc} {c}")),
        (None, Some(c)) => Some(c.to_owned()),
        _ => None,
    };
    let mut parts: Vec<String> = Vec::new();
    for part in [
        get("name").map(str::to_owned),
        street,
        city,
        get("country").map(str::to_owned),
    ]
    .into_iter()
    .flatten()
    {
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    parts.join(", ")
}

/// Photon GeoJSON `FeatureCollection` (search or reverse) → hits.
pub fn parse_photon(body: &Value) -> Result<Vec<GeocodeResult>, GeoError> {
    if let Some(msg) = body["message"].as_str() {
        return Err(GeoError::Provider(msg.to_owned()));
    }
    let features = body["features"]
        .as_array()
        .ok_or(GeoError::Malformed("photon features"))?;
    Ok(features
        .iter()
        .filter_map(|f| {
            let c = f["geometry"]["coordinates"].as_array()?;
            let p = &f["properties"];
            // extent = [min lon, max lat, max lon, min lat].
            let bbox = p["extent"].as_array().and_then(|e| {
                let v: Vec<f64> = e.iter().filter_map(Value::as_f64).collect();
                (v.len() == 4).then(|| [v[0], v[3], v[2], v[1]])
            });
            let address = p
                .as_object()?
                .iter()
                .filter(|(k, v)| v.is_string() && !k.starts_with("osm_") && *k != "type")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            Some(GeocodeResult {
                label: photon_label(p),
                lat: c.get(1)?.as_f64()?,
                lon: c.first()?.as_f64()?,
                bbox,
                kind: p["osm_value"].as_str().map(str::to_owned),
                confidence: None,
                address,
            })
        })
        .collect())
}

// ----------------------------------------------------------------- Valhalla

/// Valhalla routing profile (`costing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteProfile {
    #[default]
    Auto,
    Bicycle,
    Pedestrian,
    Truck,
}

impl RouteProfile {
    /// GraphHopper profile name (an instance only serves the profiles it
    /// was built with; others answer an error).
    fn graphhopper(self) -> &'static str {
        match self {
            RouteProfile::Auto => "car",
            RouteProfile::Bicycle => "bike",
            RouteProfile::Pedestrian => "foot",
            RouteProfile::Truck => "truck",
        }
    }

    fn costing(self) -> &'static str {
        match self {
            RouteProfile::Auto => "auto",
            RouteProfile::Bicycle => "bicycle",
            RouteProfile::Pedestrian => "pedestrian",
            RouteProfile::Truck => "truck",
        }
    }
}

/// `POST {base}/route` URL and JSON body. `points` are `(lat, lon)`, at
/// least two.
pub fn valhalla_route_request(
    base: &str,
    points: &[(f64, f64)],
    profile: RouteProfile,
    language: Option<&str>,
) -> Result<(Url, Value), GeoError> {
    if points.len() < 2 {
        return Err(GeoError::Malformed("route needs two points"));
    }
    let locations: Vec<Value> = points
        .iter()
        .map(|(lat, lon)| json!({ "lat": lat, "lon": lon }))
        .collect();
    let mut directions = json!({ "units": "kilometers" });
    if let Some(lang) = language {
        directions["language"] = json!(lang);
    }
    let body = json!({
        "locations": locations,
        "costing": profile.costing(),
        "directions_options": directions,
    });
    Ok((endpoint(base, "route")?, body))
}

/// Decodes an encoded polyline into `[lon, lat]` pairs. Valhalla uses
/// precision 6, OSRM/Google precision 5.
pub fn decode_polyline(encoded: &str, precision: u32) -> Result<Vec<[f64; 2]>, GeoError> {
    let factor = 10f64.powi(precision as i32);
    let bytes = encoded.as_bytes();
    let (mut i, mut lat, mut lon) = (0usize, 0i64, 0i64);
    let mut out = Vec::new();
    let next = |i: &mut usize| -> Result<i64, GeoError> {
        let (mut result, mut shift) = (0i64, 0u32);
        loop {
            let b = *bytes.get(*i).ok_or(GeoError::Malformed("polyline"))? as i64 - 63;
            *i += 1;
            if !(0..64).contains(&b) || shift > 60 {
                return Err(GeoError::Malformed("polyline"));
            }
            result |= (b & 0x1f) << shift;
            shift += 5;
            if b < 0x20 {
                break;
            }
        }
        Ok(if result & 1 != 0 {
            !(result >> 1)
        } else {
            result >> 1
        })
    };
    while i < bytes.len() {
        lat += next(&mut i)?;
        lon += next(&mut i)?;
        out.push([lon as f64 / factor, lat as f64 / factor]);
    }
    Ok(out)
}

/// `/route` body → [`Route`]. Legs are concatenated; step indices are
/// shifted into the joined geometry.
pub fn parse_valhalla_route(body: &Value) -> Result<Route, GeoError> {
    if let Some(err) = body["error"].as_str() {
        return Err(GeoError::Provider(err.to_owned()));
    }
    let trip = &body["trip"];
    let legs = trip["legs"]
        .as_array()
        .ok_or(GeoError::Malformed("valhalla trip.legs"))?;
    // Units are kilometers unless the response says miles.
    let to_m = if trip["units"].as_str() == Some("miles") {
        1_609.344
    } else {
        1_000.0
    };
    let mut coords: Vec<[f64; 2]> = Vec::new();
    let mut steps = Vec::new();
    for leg in legs {
        let shape = leg["shape"]
            .as_str()
            .ok_or(GeoError::Malformed("valhalla leg.shape"))?;
        let mut points = decode_polyline(shape, 6)?;
        // Consecutive legs share their junction point.
        let offset = if coords.is_empty() {
            0
        } else {
            points.remove(0);
            coords.len() - 1
        };
        for m in leg["maneuvers"].as_array().into_iter().flatten() {
            steps.push(RouteStep {
                instruction: m["instruction"].as_str().unwrap_or_default().to_owned(),
                distance_m: m["length"].as_f64().unwrap_or(0.0) * to_m,
                duration_s: m["time"].as_f64().unwrap_or(0.0),
                start_index: offset + m["begin_shape_index"].as_u64().unwrap_or(0) as usize,
            });
        }
        coords.extend(points);
    }
    let summary = &trip["summary"];
    Ok(Route {
        geometry: json!({ "type": "LineString", "coordinates": coords }),
        distance_m: summary["length"].as_f64().unwrap_or(0.0) * to_m,
        duration_s: summary["time"].as_f64().unwrap_or(0.0),
        steps,
    })
}

// -------------------------------------------------------------- GraphHopper

/// `GET {base}/route` URL. `points` are `(lat, lon)`, at least two.
pub fn graphhopper_route_url(
    base: &str,
    points: &[(f64, f64)],
    profile: RouteProfile,
    language: Option<&str>,
) -> Result<Url, GeoError> {
    if points.len() < 2 {
        return Err(GeoError::Malformed("route needs two points"));
    }
    let mut url = endpoint(base, "route")?;
    {
        let mut q = url.query_pairs_mut();
        for (lat, lon) in points {
            q.append_pair("point", &format!("{lat},{lon}"));
        }
        q.append_pair("profile", profile.graphhopper())
            .append_pair("points_encoded", "false");
        if let Some(lang) = language {
            q.append_pair("locale", lang);
        }
    }
    Ok(url)
}

/// `/route` body → [`Route`] (first path). Times are milliseconds.
pub fn parse_graphhopper_route(body: &Value) -> Result<Route, GeoError> {
    if let Some(msg) = body["message"].as_str() {
        return Err(GeoError::Provider(msg.to_owned()));
    }
    let path = body["paths"]
        .as_array()
        .and_then(|p| p.first())
        .ok_or(GeoError::Malformed("graphhopper paths"))?;
    let coords = path["points"]["coordinates"]
        .as_array()
        .ok_or(GeoError::Malformed("graphhopper points"))?;
    let steps = path["instructions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| RouteStep {
            instruction: i["text"].as_str().unwrap_or_default().to_owned(),
            distance_m: i["distance"].as_f64().unwrap_or(0.0),
            duration_s: i["time"].as_f64().unwrap_or(0.0) / 1_000.0,
            start_index: i["interval"][0].as_u64().unwrap_or(0) as usize,
        })
        .collect();
    Ok(Route {
        geometry: json!({ "type": "LineString", "coordinates": coords }),
        distance_m: path["distance"].as_f64().unwrap_or(0.0),
        duration_s: path["time"].as_f64().unwrap_or(0.0) / 1_000.0,
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_keeps_base_path_and_rejects_other_schemes() {
        let u = nominatim_search_url("https://geo.example/nominatim/", "10 rue X", 5, &[]).unwrap();
        assert_eq!(u.path(), "/nominatim/search");
        assert!(u.query().unwrap().contains("q=10+rue+X"));
        assert_eq!(
            nominatim_search_url("file:///etc", "x", 1, &[]),
            Err(GeoError::BadBaseUrl)
        );
    }

    #[test]
    fn nominatim_search_and_reverse_miss() {
        let body = json!([{
            "display_name": "Tour Eiffel, Paris",
            "lat": "48.8582599", "lon": "2.2945006",
            "boundingbox": ["48.8574753", "48.8590453", "2.2933119", "2.2956897"],
            "type": "attraction", "importance": 0.73,
            "address": { "city": "Paris" }
        }]);
        let hits = parse_nominatim(&body).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].lat, 48.8582599);
        assert_eq!(
            hits[0].bbox,
            Some([2.2933119, 48.8574753, 2.2956897, 48.8590453])
        );
        assert_eq!(hits[0].address["city"], "Paris");
        assert!(parse_nominatim(&json!({ "error": "Unable to geocode" }))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn polyline_reference_vector() {
        // Google's reference example, precision 5.
        let pts = decode_polyline("_p~iF~ps|U_ulLnnqC_mqNvxq`@", 5).unwrap();
        assert_eq!(
            pts,
            vec![[-120.2, 38.5], [-120.95, 40.7], [-126.453, 43.252]]
        );
        assert!(decode_polyline("_p~iF~ps|U_", 5).is_err());
    }

    #[test]
    fn valhalla_two_legs_join_and_error() {
        // Precision 6, [lon, lat]: (0,0)→(1e-5,0) then (1e-5,0)→(2e-5,0).
        let leg = |shape: &str| {
            json!({ "shape": shape, "maneuvers": [
                { "instruction": "Go", "length": 0.5, "time": 30.0, "begin_shape_index": 0 },
                { "instruction": "Arrive", "length": 0.0, "time": 0.0, "begin_shape_index": 1 }
            ]})
        };
        let body = json!({ "trip": {
            "legs": [leg("???S"), leg("?S?S")],
            "summary": { "length": 1.0, "time": 60.0 },
            "units": "kilometers"
        }});
        let r = parse_valhalla_route(&body).unwrap();
        assert_eq!(r.geometry["coordinates"].as_array().unwrap().len(), 3);
        assert_eq!(r.distance_m, 1_000.0);
        assert_eq!(r.steps[2].start_index, 1);
        assert_eq!(
            parse_valhalla_route(&json!({ "error": "No path", "error_code": 442 })),
            Err(GeoError::Provider("No path".into()))
        );
        assert!(
            valhalla_route_request("http://v", &[(0.0, 0.0)], RouteProfile::Auto, None).is_err()
        );
    }

    #[test]
    fn photon_search_reverse_and_error() {
        // Trimmed answer of the alpine-box Photon (2026-10-10).
        let body = json!({ "type": "FeatureCollection", "features": [{
            "type": "Feature",
            "properties": {
                "osm_type": "W", "osm_id": 5013364, "osm_key": "man_made", "osm_value": "tower",
                "type": "house", "housenumber": "5", "name": "Tour Eiffel",
                "street": "Avenue Anatole France", "city": "Paris", "postcode": "75007",
                "country": "France", "countrycode": "FR",
                "extent": [2.2933119, 48.8590453, 2.2956897, 48.8574753]
            },
            "geometry": { "type": "Point", "coordinates": [2.2945006, 48.8582599] }
        }]});
        let hits = parse_photon(&body).unwrap();
        assert_eq!(
            hits[0].label,
            "Tour Eiffel, 5 Avenue Anatole France, 75007 Paris, France"
        );
        assert_eq!((hits[0].lat, hits[0].lon), (48.8582599, 2.2945006));
        assert_eq!(
            hits[0].bbox,
            Some([2.2933119, 48.8574753, 2.2956897, 48.8590453])
        );
        assert_eq!(hits[0].kind.as_deref(), Some("tower"));
        assert_eq!(hits[0].address["city"], "Paris");
        assert!(!hits[0].address.contains_key("osm_key"));
        let u = photon_search_url("http://192.168.1.208/photon", "tour eiffel", 3, &[]).unwrap();
        assert_eq!(u.path(), "/photon/api");
        assert!(parse_photon(&json!({ "message": "bad request" })).is_err());
    }

    #[test]
    fn graphhopper_route_and_error() {
        let body = json!({ "paths": [{
            "distance": 4295.887, "time": 519338,
            "points": { "type": "LineString", "coordinates": [[2.293472, 48.859034], [2.295619, 48.860542]] },
            "instructions": [
                { "text": "Continue", "distance": 250.0, "time": 30000, "interval": [0, 1] },
                { "text": "Arrive", "distance": 0.0, "time": 0, "interval": [1, 1] }
            ]
        }]});
        let r = parse_graphhopper_route(&body).unwrap();
        assert_eq!(r.distance_m, 4295.887);
        assert_eq!(r.duration_s, 519.338);
        assert_eq!(r.steps[0].duration_s, 30.0);
        assert_eq!(r.steps[1].start_index, 1);
        let u = graphhopper_route_url(
            "http://192.168.1.208/graphhopper",
            &[(48.8584, 2.2945), (48.8606, 2.3376)],
            RouteProfile::Auto,
            Some("fr"),
        )
        .unwrap();
        assert_eq!(u.path(), "/graphhopper/route");
        assert!(u
            .query()
            .unwrap()
            .contains("point=48.8584%2C2.2945&point=48.8606%2C2.3376&profile=car"));
        assert_eq!(
            parse_graphhopper_route(&json!({ "message": "Cannot find point 0" })),
            Err(GeoError::Provider("Cannot find point 0".into()))
        );
    }
}
