//! Weather source node contract (D140, `docs/architecture/home-dashboards.md`):
//! config, allowlisted provider URLs and the normalization of each provider
//! response into three stable payloads (current, daily, hourly). Pure and
//! wasm-safe: the HTTP call lives in `pnex-node-weather`.
//!
//! The URL is always built here from numeric coordinates: the user never
//! types a host (R8). Payloads are flat objects of numbers (plus a few
//! strings and the `days` / `hours` arrays) so `memory-write` and
//! `pnex-metric` store them as is.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// Output ports of the node.
pub const WEATHER_PORT_COUNT: usize = 3;
/// Bounds of the refresh interval (minutes): providers ask for no more than
/// a call every few minutes per location.
pub const WEATHER_INTERVAL_MIN: u32 = 10;
pub const WEATHER_INTERVAL_MAX: u32 = 1_440;
/// Days of the daily payload, hours of the hourly payload.
pub const WEATHER_DAYS: usize = 7;
pub const WEATHER_HOURS: usize = 48;
/// Hours flattened as `h{n}_*` fields (the full list stays in `hours`).
pub const WEATHER_FLAT_HOURS: usize = 24;

/// Forecast provider (allowlist).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherProvider {
    /// MET Norway Locationforecast 2.0: free, CC BY 4.0 (commercial use
    /// allowed with attribution), identified User-Agent required.
    #[default]
    MetNorway,
    /// Open-Meteo: free for non-commercial use only.
    OpenMeteo,
}

impl WeatherProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            WeatherProvider::MetNorway => "met_norway",
            WeatherProvider::OpenMeteo => "open_meteo",
        }
    }

    /// Attribution required by the provider's terms.
    pub fn attribution(self) -> &'static str {
        match self {
            WeatherProvider::MetNorway => "Weather data from MET Norway (CC BY 4.0)",
            WeatherProvider::OpenMeteo => "Weather data by Open-Meteo.com (CC BY 4.0)",
        }
    }
}

fn default_interval() -> u32 {
    30
}

fn default_true() -> bool {
    true
}

/// Configuration of the `weather` node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherConfig {
    #[serde(default)]
    pub provider: WeatherProvider,
    pub latitude: f64,
    pub longitude: f64,
    /// Refresh interval in minutes.
    #[serde(default = "default_interval")]
    pub interval_min: u32,
    /// Fetch once at engine start / redeploy (else wait one interval).
    #[serde(default = "default_true")]
    pub emit_on_start: bool,
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            provider: WeatherProvider::default(),
            latitude: 48.8566,
            longitude: 2.3522,
            interval_min: default_interval(),
            emit_on_start: true,
        }
    }
}

impl WeatherConfig {
    /// Structural check shared by the save validation and the runtime.
    pub fn check(&self) -> Option<(&'static str, String)> {
        if !self.latitude.is_finite() || !(-90.0..=90.0).contains(&self.latitude) {
            return Some((
                "weather_latitude",
                "latitude must be within -90..=90".into(),
            ));
        }
        if !self.longitude.is_finite() || !(-180.0..=180.0).contains(&self.longitude) {
            return Some((
                "weather_longitude",
                "longitude must be within -180..=180".into(),
            ));
        }
        if !(WEATHER_INTERVAL_MIN..=WEATHER_INTERVAL_MAX).contains(&self.interval_min) {
            return Some((
                "weather_interval",
                format!(
                    "the refresh interval must be within {WEATHER_INTERVAL_MIN}..={WEATHER_INTERVAL_MAX} minutes"
                ),
            ));
        }
        None
    }

    /// Request URL (fixed host, coordinates rounded to 4 decimals as the
    /// MET Norway terms ask).
    pub fn url(&self) -> String {
        let (lat, lon) = (round4(self.latitude), round4(self.longitude));
        match self.provider {
            WeatherProvider::MetNorway => format!(
                "https://api.met.no/weatherapi/locationforecast/2.0/complete?lat={lat}&lon={lon}"
            ),
            WeatherProvider::OpenMeteo => format!(
                "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}\
                 &current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,\
                 precipitation,weather_code,cloud_cover,pressure_msl,wind_speed_10m,\
                 wind_direction_10m,wind_gusts_10m\
                 &hourly=temperature_2m,precipitation_probability,precipitation,weather_code,\
                 wind_speed_10m,is_day\
                 &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_sum,\
                 precipitation_probability_max,wind_speed_10m_max,uv_index_max,sunrise,sunset\
                 &timezone=auto&forecast_days={WEATHER_DAYS}&wind_speed_unit=kmh"
            ),
        }
    }
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

/// Normalized sky condition, shared by every provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherCondition {
    Clear,
    PartlyCloudy,
    Cloudy,
    Fog,
    Drizzle,
    Rain,
    HeavyRain,
    Sleet,
    Snow,
    Thunderstorm,
}

impl WeatherCondition {
    pub const ALL: [WeatherCondition; 10] = [
        WeatherCondition::Clear,
        WeatherCondition::PartlyCloudy,
        WeatherCondition::Cloudy,
        WeatherCondition::Fog,
        WeatherCondition::Drizzle,
        WeatherCondition::Rain,
        WeatherCondition::HeavyRain,
        WeatherCondition::Sleet,
        WeatherCondition::Snow,
        WeatherCondition::Thunderstorm,
    ];

    /// Numeric code (stored in memory / O2 as `condition_code`).
    pub fn code(self) -> u8 {
        WeatherCondition::ALL
            .iter()
            .position(|c| *c == self)
            .unwrap_or(0) as u8
    }

    pub fn from_code(code: f64) -> Option<WeatherCondition> {
        if code.fract() != 0.0 || code < 0.0 {
            return None;
        }
        WeatherCondition::ALL.get(code as usize).copied()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WeatherCondition::Clear => "clear",
            WeatherCondition::PartlyCloudy => "partly_cloudy",
            WeatherCondition::Cloudy => "cloudy",
            WeatherCondition::Fog => "fog",
            WeatherCondition::Drizzle => "drizzle",
            WeatherCondition::Rain => "rain",
            WeatherCondition::HeavyRain => "heavy_rain",
            WeatherCondition::Sleet => "sleet",
            WeatherCondition::Snow => "snow",
            WeatherCondition::Thunderstorm => "thunderstorm",
        }
    }

    /// Home icon catalog id (D136).
    pub fn icon(self, is_day: bool) -> &'static str {
        match self {
            WeatherCondition::Clear if is_day => "home-sun",
            WeatherCondition::Clear => "home-moon",
            WeatherCondition::PartlyCloudy => "home-partly-cloudy",
            WeatherCondition::Cloudy => "home-cloud",
            WeatherCondition::Fog => "home-fog",
            WeatherCondition::Drizzle | WeatherCondition::Rain | WeatherCondition::HeavyRain => {
                "home-rain"
            }
            WeatherCondition::Sleet | WeatherCondition::Snow => "home-snow",
            WeatherCondition::Thunderstorm => "home-storm",
        }
    }

    /// WMO weather interpretation code (Open-Meteo).
    pub fn from_wmo(code: i64) -> WeatherCondition {
        match code {
            0 => WeatherCondition::Clear,
            1 | 2 => WeatherCondition::PartlyCloudy,
            3 => WeatherCondition::Cloudy,
            45 | 48 => WeatherCondition::Fog,
            51..=57 => WeatherCondition::Drizzle,
            65 | 82 => WeatherCondition::HeavyRain,
            66 | 67 => WeatherCondition::Sleet,
            71..=77 | 85 | 86 => WeatherCondition::Snow,
            95..=99 => WeatherCondition::Thunderstorm,
            _ => WeatherCondition::Rain,
        }
    }

    /// MET Norway symbol code (`partlycloudy_day`, `heavyrainandthunder`…).
    pub fn from_met_symbol(symbol: &str) -> WeatherCondition {
        let base = symbol
            .split('_')
            .next()
            .unwrap_or(symbol)
            .to_ascii_lowercase();
        if base.contains("thunder") {
            WeatherCondition::Thunderstorm
        } else if base.contains("sleet") {
            WeatherCondition::Sleet
        } else if base.contains("snow") {
            WeatherCondition::Snow
        } else if base.starts_with("heavyrain") {
            WeatherCondition::HeavyRain
        } else if base.starts_with("lightrain") {
            WeatherCondition::Drizzle
        } else if base.contains("rain") {
            WeatherCondition::Rain
        } else if base == "fog" {
            WeatherCondition::Fog
        } else if base == "cloudy" {
            WeatherCondition::Cloudy
        } else if base == "fair" || base == "partlycloudy" {
            WeatherCondition::PartlyCloudy
        } else {
            WeatherCondition::Clear
        }
    }
}

/// Is a MET Norway symbol a night one?
fn met_is_day(symbol: &str) -> bool {
    !symbol.ends_with("_night") && !symbol.ends_with("_polartwilight")
}

/// Apparent temperature (Steadman, shade, °C) from temperature (°C),
/// relative humidity (%) and wind (m/s).
pub fn apparent_temperature(t: f64, rh: f64, wind_ms: f64) -> f64 {
    let e = rh / 100.0 * 6.105 * (17.27 * t / (237.7 + t)).exp();
    t + 0.33 * e - 0.70 * wind_ms - 4.00
}

/// The three normalized payloads of one fetch.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherPayloads {
    pub current: Value,
    pub daily: Value,
    pub hourly: Value,
}

/// Why a provider response cannot be normalized.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherParseError(pub String);

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|x| x.is_finite())
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn put(obj: &mut Map<String, Value>, key: &str, v: Option<f64>) {
    if let Some(v) = v {
        obj.insert(key.to_string(), json!(round1(v)));
    }
}

/// Flattens the first rows of `rows` as `{prefix}{i}_{field}` numbers.
fn flatten(
    out: &mut Map<String, Value>,
    prefix: &str,
    rows: &[Map<String, Value>],
    n: usize,
    fields: &[&str],
) {
    for (i, row) in rows.iter().take(n).enumerate() {
        for f in fields {
            if let Some(v) = row.get(*f).filter(|v| v.is_number()) {
                out.insert(format!("{prefix}{i}_{f}"), v.clone());
            }
        }
    }
}

const DAY_FIELDS: &[&str] = &[
    "t_min",
    "t_max",
    "precipitation",
    "precipitation_probability",
    "wind_max",
    "uv_max",
    "condition_code",
];
const HOUR_FIELDS: &[&str] = &["temperature", "precipitation", "condition_code"];

fn condition_fields(row: &mut Map<String, Value>, c: WeatherCondition, is_day: bool) {
    row.insert("condition".into(), json!(c.as_str()));
    row.insert("condition_code".into(), json!(c.code()));
    row.insert("icon".into(), json!(c.icon(is_day)));
}

fn assemble(
    provider: WeatherProvider,
    mut current: Map<String, Value>,
    days: Vec<Map<String, Value>>,
    hours: Vec<Map<String, Value>>,
) -> WeatherPayloads {
    let p = json!(provider.as_str());
    current.insert("provider".into(), p.clone());
    let mut daily = Map::new();
    daily.insert("provider".into(), p.clone());
    flatten(&mut daily, "d", &days, WEATHER_DAYS, DAY_FIELDS);
    daily.insert(
        "days".into(),
        Value::Array(days.into_iter().map(Value::Object).collect()),
    );
    let mut hourly = Map::new();
    hourly.insert("provider".into(), p);
    flatten(&mut hourly, "h", &hours, WEATHER_FLAT_HOURS, HOUR_FIELDS);
    hourly.insert(
        "hours".into(),
        Value::Array(hours.into_iter().map(Value::Object).collect()),
    );
    WeatherPayloads {
        current: Value::Object(current),
        daily: Value::Object(daily),
        hourly: Value::Object(hourly),
    }
}

/// Normalizes a MET Norway Locationforecast 2.0 `complete` response.
/// Daily rows group the timeseries by UTC date.
pub fn parse_met_norway(body: &Value) -> Result<WeatherPayloads, WeatherParseError> {
    let series = body
        .pointer("/properties/timeseries")
        .and_then(Value::as_array)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| WeatherParseError("no timeseries in the MET Norway response".into()))?;
    let instant = |e: &Value, k: &str| num(e.pointer(&format!("/data/instant/details/{k}")));
    let next =
        |e: &Value, period: &str, k: &str| num(e.pointer(&format!("/data/{period}/details/{k}")));
    let symbol = |e: &Value| {
        ["next_1_hours", "next_6_hours", "next_12_hours"]
            .iter()
            .find_map(|p| e.pointer(&format!("/data/{p}/summary/symbol_code")))
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    let first = &series[0];
    let sym = symbol(first).unwrap_or_default();
    let cond = WeatherCondition::from_met_symbol(&sym);
    let is_day = met_is_day(&sym);
    let t = instant(first, "air_temperature");
    let rh = instant(first, "relative_humidity");
    let wind_ms = instant(first, "wind_speed");
    let mut current = Map::new();
    current.insert(
        "time".into(),
        first.get("time").cloned().unwrap_or(Value::Null),
    );
    put(&mut current, "temperature", t);
    put(
        &mut current,
        "feels_like",
        match (t, rh, wind_ms) {
            (Some(t), Some(rh), Some(w)) => Some(apparent_temperature(t, rh, w)),
            _ => None,
        },
    );
    put(&mut current, "humidity", rh);
    put(
        &mut current,
        "pressure",
        instant(first, "air_pressure_at_sea_level"),
    );
    put(&mut current, "wind_speed", wind_ms.map(|w| w * 3.6));
    put(
        &mut current,
        "wind_gust",
        instant(first, "wind_speed_of_gust").map(|w| w * 3.6),
    );
    put(
        &mut current,
        "wind_direction",
        instant(first, "wind_from_direction"),
    );
    put(
        &mut current,
        "cloud_cover",
        instant(first, "cloud_area_fraction"),
    );
    put(
        &mut current,
        "uv_index",
        instant(first, "ultraviolet_index_clear_sky"),
    );
    put(
        &mut current,
        "precipitation",
        next(first, "next_1_hours", "precipitation_amount"),
    );
    put(
        &mut current,
        "precipitation_probability",
        next(first, "next_1_hours", "probability_of_precipitation"),
    );
    current.insert("is_day".into(), json!(is_day));
    condition_fields(&mut current, cond, is_day);

    // Hourly rows: entries that carry a 1-hour summary.
    let hours: Vec<Map<String, Value>> = series
        .iter()
        .filter(|e| e.pointer("/data/next_1_hours").is_some())
        .take(WEATHER_HOURS)
        .map(|e| {
            let sym = symbol(e).unwrap_or_default();
            let mut row = Map::new();
            row.insert("time".into(), e.get("time").cloned().unwrap_or(Value::Null));
            put(&mut row, "temperature", instant(e, "air_temperature"));
            put(
                &mut row,
                "precipitation",
                next(e, "next_1_hours", "precipitation_amount"),
            );
            put(
                &mut row,
                "precipitation_probability",
                next(e, "next_1_hours", "probability_of_precipitation"),
            );
            put(
                &mut row,
                "wind_speed",
                instant(e, "wind_speed").map(|w| w * 3.6),
            );
            condition_fields(
                &mut row,
                WeatherCondition::from_met_symbol(&sym),
                met_is_day(&sym),
            );
            row
        })
        .collect();

    // Daily rows: group by UTC date (first 10 chars of the RFC 3339 time).
    let mut days: Vec<(String, Vec<&Value>)> = Vec::new();
    for e in series {
        let Some(date) = e
            .get("time")
            .and_then(Value::as_str)
            .and_then(|t| t.get(..10))
        else {
            continue;
        };
        match days.last_mut() {
            Some((d, list)) if d == date => list.push(e),
            _ => days.push((date.to_string(), vec![e])),
        }
    }
    let days: Vec<Map<String, Value>> = days
        .into_iter()
        .take(WEATHER_DAYS)
        .map(|(date, entries)| {
            let temps: Vec<f64> = entries
                .iter()
                .filter_map(|e| instant(e, "air_temperature"))
                .collect();
            let fold = |init: f64, f: fn(f64, f64) -> f64| {
                (!temps.is_empty()).then(|| temps.iter().copied().fold(init, f))
            };
            // Precipitation: hourly amounts when present, else 6-hour ones.
            let hourly: Vec<f64> = entries
                .iter()
                .filter_map(|e| next(e, "next_1_hours", "precipitation_amount"))
                .collect();
            let precipitation = if hourly.is_empty() {
                entries
                    .iter()
                    .filter_map(|e| next(e, "next_6_hours", "precipitation_amount"))
                    .sum::<f64>()
            } else {
                hourly.iter().sum::<f64>()
            };
            let prob = entries
                .iter()
                .filter_map(|e| {
                    next(e, "next_1_hours", "probability_of_precipitation")
                        .or_else(|| next(e, "next_6_hours", "probability_of_precipitation"))
                })
                .fold(None, |acc: Option<f64>, v| {
                    Some(acc.map_or(v, |a| a.max(v)))
                });
            let wind = entries
                .iter()
                .filter_map(|e| instant(e, "wind_speed"))
                .fold(None, |acc: Option<f64>, v| {
                    Some(acc.map_or(v, |a| a.max(v)))
                });
            let uv = entries
                .iter()
                .filter_map(|e| instant(e, "ultraviolet_index_clear_sky"))
                .fold(None, |acc: Option<f64>, v| {
                    Some(acc.map_or(v, |a| a.max(v)))
                });
            // Day condition: the symbol around midday, else the first one.
            let midday = entries
                .iter()
                .find(|e| {
                    e.get("time")
                        .and_then(Value::as_str)
                        .is_some_and(|t| t.get(11..13).is_some_and(|h| ("11"..="13").contains(&h)))
                })
                .or(entries.first())
                .and_then(|e| symbol(e))
                .unwrap_or_default();
            let mut row = Map::new();
            row.insert("date".into(), json!(date));
            put(&mut row, "t_min", fold(f64::INFINITY, f64::min));
            put(&mut row, "t_max", fold(f64::NEG_INFINITY, f64::max));
            put(&mut row, "precipitation", Some(precipitation));
            put(&mut row, "precipitation_probability", prob);
            put(&mut row, "wind_max", wind.map(|w| w * 3.6));
            put(&mut row, "uv_max", uv);
            condition_fields(&mut row, WeatherCondition::from_met_symbol(&midday), true);
            row
        })
        .collect();

    Ok(assemble(WeatherProvider::MetNorway, current, days, hours))
}

/// Normalizes an Open-Meteo `/v1/forecast` response (URL of
/// [`WeatherConfig::url`]: km/h winds, local time zone).
pub fn parse_open_meteo(body: &Value) -> Result<WeatherPayloads, WeatherParseError> {
    let cur = body
        .get("current")
        .ok_or_else(|| WeatherParseError("no current block in the Open-Meteo response".into()))?;
    let c = |k: &str| num(cur.get(k));
    let is_day = c("is_day").is_none_or(|d| d >= 0.5);
    let cond = WeatherCondition::from_wmo(c("weather_code").unwrap_or(0.0) as i64);
    let mut current = Map::new();
    current.insert(
        "time".into(),
        cur.get("time").cloned().unwrap_or(Value::Null),
    );
    put(&mut current, "temperature", c("temperature_2m"));
    put(&mut current, "feels_like", c("apparent_temperature"));
    put(&mut current, "humidity", c("relative_humidity_2m"));
    put(&mut current, "pressure", c("pressure_msl"));
    put(&mut current, "wind_speed", c("wind_speed_10m"));
    put(&mut current, "wind_gust", c("wind_gusts_10m"));
    put(&mut current, "wind_direction", c("wind_direction_10m"));
    put(&mut current, "cloud_cover", c("cloud_cover"));
    put(&mut current, "precipitation", c("precipitation"));
    current.insert("is_day".into(), json!(is_day));
    condition_fields(&mut current, cond, is_day);

    let col = |block: &str, k: &str| -> Vec<Value> {
        body.pointer(&format!("/{block}/{k}"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let at = |v: &[Value], i: usize| num(v.get(i));

    let (h_time, h_t, h_pp, h_p, h_code, h_w, h_day) = (
        col("hourly", "time"),
        col("hourly", "temperature_2m"),
        col("hourly", "precipitation_probability"),
        col("hourly", "precipitation"),
        col("hourly", "weather_code"),
        col("hourly", "wind_speed_10m"),
        col("hourly", "is_day"),
    );
    // Hourly rows from the current hour on.
    let now = cur
        .get("time")
        .and_then(Value::as_str)
        .and_then(|t| t.get(..13))
        .unwrap_or("");
    let start = h_time
        .iter()
        .position(|t| t.as_str().is_some_and(|t| t.get(..13).unwrap_or("") >= now))
        .unwrap_or(0);
    let hours: Vec<Map<String, Value>> = (start..h_time.len())
        .take(WEATHER_HOURS)
        .map(|i| {
            let mut row = Map::new();
            row.insert("time".into(), h_time[i].clone());
            put(&mut row, "temperature", at(&h_t, i));
            put(&mut row, "precipitation", at(&h_p, i));
            put(&mut row, "precipitation_probability", at(&h_pp, i));
            put(&mut row, "wind_speed", at(&h_w, i));
            let code = at(&h_code, i).unwrap_or(0.0) as i64;
            let day = at(&h_day, i).is_none_or(|d| d >= 0.5);
            condition_fields(&mut row, WeatherCondition::from_wmo(code), day);
            row
        })
        .collect();

    let (d_time, d_code, d_max, d_min, d_p, d_pp, d_w, d_uv, d_rise, d_set) = (
        col("daily", "time"),
        col("daily", "weather_code"),
        col("daily", "temperature_2m_max"),
        col("daily", "temperature_2m_min"),
        col("daily", "precipitation_sum"),
        col("daily", "precipitation_probability_max"),
        col("daily", "wind_speed_10m_max"),
        col("daily", "uv_index_max"),
        col("daily", "sunrise"),
        col("daily", "sunset"),
    );
    let days: Vec<Map<String, Value>> = (0..d_time.len().min(WEATHER_DAYS))
        .map(|i| {
            let mut row = Map::new();
            row.insert("date".into(), d_time[i].clone());
            put(&mut row, "t_min", at(&d_min, i));
            put(&mut row, "t_max", at(&d_max, i));
            put(&mut row, "precipitation", at(&d_p, i));
            put(&mut row, "precipitation_probability", at(&d_pp, i));
            put(&mut row, "wind_max", at(&d_w, i));
            put(&mut row, "uv_max", at(&d_uv, i));
            if let Some(v) = d_rise.get(i).filter(|v| v.is_string()) {
                row.insert("sunrise".into(), v.clone());
            }
            if let Some(v) = d_set.get(i).filter(|v| v.is_string()) {
                row.insert("sunset".into(), v.clone());
            }
            let code = at(&d_code, i).unwrap_or(0.0) as i64;
            condition_fields(&mut row, WeatherCondition::from_wmo(code), true);
            row
        })
        .collect();

    Ok(assemble(WeatherProvider::OpenMeteo, current, days, hours))
}

/// Normalizes the response of the configured provider.
pub fn parse_weather(
    provider: WeatherProvider,
    body: &Value,
) -> Result<WeatherPayloads, WeatherParseError> {
    match provider {
        WeatherProvider::MetNorway => parse_met_norway(body),
        WeatherProvider::OpenMeteo => parse_open_meteo(body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn met_entry(time: &str, t: f64, symbol: &str, rain: f64) -> Value {
        json!({
            "time": time,
            "data": {
                "instant": {"details": {
                    "air_temperature": t, "relative_humidity": 70.0, "wind_speed": 5.0,
                    "wind_speed_of_gust": 9.0, "wind_from_direction": 220.0,
                    "air_pressure_at_sea_level": 1012.3, "cloud_area_fraction": 80.0,
                    "ultraviolet_index_clear_sky": 2.0
                }},
                "next_1_hours": {
                    "summary": {"symbol_code": symbol},
                    "details": {"precipitation_amount": rain, "probability_of_precipitation": 40.0}
                }
            }
        })
    }

    #[test]
    fn config_bounds_and_allowlisted_urls() {
        let mut c = WeatherConfig::default();
        assert!(c.check().is_none());
        c.latitude = 48.856_613_7;
        assert!(c.url().starts_with(
            "https://api.met.no/weatherapi/locationforecast/2.0/complete?lat=48.8566&lon="
        ));
        c.provider = WeatherProvider::OpenMeteo;
        assert!(c
            .url()
            .starts_with("https://api.open-meteo.com/v1/forecast?latitude=48.8566"));
        c.latitude = 91.0;
        assert_eq!(c.check().unwrap().0, "weather_latitude");
        c.latitude = 0.0;
        c.longitude = f64::NAN;
        assert_eq!(c.check().unwrap().0, "weather_longitude");
        c.longitude = 0.0;
        c.interval_min = 5;
        assert_eq!(c.check().unwrap().0, "weather_interval");
        let parsed: WeatherConfig =
            serde_json::from_str(r#"{"latitude":1,"longitude":2}"#).unwrap();
        assert_eq!(parsed.interval_min, 30);
        assert!(parsed.emit_on_start);
    }

    #[test]
    fn conditions_map_from_both_providers() {
        use WeatherCondition::*;
        let met = [
            ("clearsky_day", Clear),
            ("fair_night", PartlyCloudy),
            ("partlycloudy_day", PartlyCloudy),
            ("cloudy", Cloudy),
            ("fog", Fog),
            ("lightrain", Drizzle),
            ("rainshowers_day", Rain),
            ("heavyrain", HeavyRain),
            ("lightsleet", Sleet),
            ("heavysnowshowers_night", Snow),
            ("rainandthunder", Thunderstorm),
        ];
        for (s, c) in met {
            assert_eq!(WeatherCondition::from_met_symbol(s), c, "{s}");
        }
        let wmo = [
            (0, Clear),
            (2, PartlyCloudy),
            (3, Cloudy),
            (45, Fog),
            (53, Drizzle),
            (63, Rain),
            (65, HeavyRain),
            (67, Sleet),
            (75, Snow),
            (96, Thunderstorm),
        ];
        for (code, c) in wmo {
            assert_eq!(WeatherCondition::from_wmo(code), c, "{code}");
        }
        for c in WeatherCondition::ALL {
            assert_eq!(WeatherCondition::from_code(f64::from(c.code())), Some(c));
            assert!(crate::valid_symbol_id(c.icon(true)) && crate::valid_symbol_id(c.icon(false)));
        }
        assert_eq!(WeatherCondition::Clear.icon(false), "home-moon");
        assert_eq!(WeatherCondition::from_code(1.5), None);
    }

    #[test]
    fn met_norway_response_is_normalized() {
        let body = json!({"properties": {"timeseries": [
            met_entry("2026-10-04T10:00:00Z", 14.0, "partlycloudy_day", 0.0),
            met_entry("2026-10-04T12:00:00Z", 17.5, "lightrain", 0.4),
            met_entry("2026-10-04T22:00:00Z", 9.0, "clearsky_night", 0.0),
            met_entry("2026-10-05T12:00:00Z", 12.0, "heavyrain", 3.0),
        ]}});
        let p = parse_met_norway(&body).unwrap();
        assert_eq!(p.current["temperature"], 14.0);
        assert_eq!(p.current["wind_speed"], 18.0, "m/s converted to km/h");
        assert_eq!(p.current["condition"], "partly_cloudy");
        assert_eq!(p.current["is_day"], true);
        assert!(p.current["feels_like"].as_f64().unwrap() < 14.0);
        assert_eq!(p.daily["d0_t_min"], 9.0);
        assert_eq!(p.daily["d0_t_max"], 17.5);
        assert_eq!(p.daily["d0_precipitation"], 0.4);
        assert_eq!(
            p.daily["d0_condition_code"],
            WeatherCondition::Drizzle.code()
        );
        assert_eq!(
            p.daily["d1_condition_code"],
            WeatherCondition::HeavyRain.code()
        );
        assert_eq!(p.daily["days"].as_array().unwrap().len(), 2);
        assert_eq!(p.hourly["h3_temperature"], 12.0);
        assert_eq!(p.hourly["provider"], "met_norway");
        assert!(parse_met_norway(&json!({})).is_err());
    }

    #[test]
    fn open_meteo_response_is_normalized() {
        let body = json!({
            "current": {
                "time": "2026-10-04T10:15", "temperature_2m": 15.2, "relative_humidity_2m": 60,
                "apparent_temperature": 13.9, "is_day": 1, "precipitation": 0.0,
                "weather_code": 3, "cloud_cover": 90, "pressure_msl": 1015.0,
                "wind_speed_10m": 12.0, "wind_direction_10m": 200, "wind_gusts_10m": 25.0
            },
            "hourly": {
                "time": ["2026-10-04T09:00", "2026-10-04T10:00", "2026-10-04T11:00"],
                "temperature_2m": [14.0, 15.0, 16.0],
                "precipitation_probability": [0, 10, 20],
                "precipitation": [0.0, 0.0, 0.2],
                "weather_code": [3, 3, 61],
                "wind_speed_10m": [10.0, 12.0, 14.0],
                "is_day": [1, 1, 1]
            },
            "daily": {
                "time": ["2026-10-04", "2026-10-05"],
                "weather_code": [61, 0],
                "temperature_2m_max": [17.0, 19.0],
                "temperature_2m_min": [9.0, 8.0],
                "precipitation_sum": [1.2, 0.0],
                "precipitation_probability_max": [60, 5],
                "wind_speed_10m_max": [25.0, 15.0],
                "uv_index_max": [3.1, 4.0],
                "sunrise": ["2026-10-04T07:58", "2026-10-05T07:59"],
                "sunset": ["2026-10-04T19:24", "2026-10-05T19:22"]
            }
        });
        let p = parse_open_meteo(&body).unwrap();
        assert_eq!(p.current["condition"], "cloudy");
        assert_eq!(p.current["feels_like"], 13.9);
        assert_eq!(
            p.hourly["h0_temperature"], 15.0,
            "starts at the current hour"
        );
        assert_eq!(p.hourly["hours"].as_array().unwrap().len(), 2);
        assert_eq!(p.daily["d1_t_max"], 19.0);
        assert_eq!(p.daily["d0_condition_code"], WeatherCondition::Rain.code());
        assert_eq!(p.daily["days"][0]["sunset"], "2026-10-04T19:24");
        assert!(parse_open_meteo(&json!({})).is_err());
    }
}
