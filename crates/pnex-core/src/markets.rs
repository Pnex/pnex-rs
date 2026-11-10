//! Primary market sources (markets-research.md D540, D541): what SEC EDGAR
//! and FRED answer, turned into import records, and the observations those
//! records hold. Pure code on hostile input: a shape it does not recognise
//! gives `None`, never a partial guess.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ontology::import::{parse_csv, table_rows};

pub mod accounts;
pub mod calendar;
pub mod chart;
pub mod decisions;
pub mod exhibit21;
pub mod history;
pub mod moves;
pub mod prediction;
pub mod same_company;

/// Field of a per-object record holding the value it was queried with.
pub const QUERIED: &str = "@queried";

/// Forms that carry results or events; insider and ownership forms stay out.
const FORMS: [&str; 10] = [
    "10-K", "10-K/A", "10-Q", "10-Q/A", "8-K", "8-K/A", "20-F", "20-F/A", "40-F", "6-K",
];

/// Reported figures kept per company, US GAAP and IFRS names: a company
/// reports hundreds of concepts, most never read.
const CONCEPTS: [&str; 30] = [
    "Revenues",
    "RevenueFromContractWithCustomerExcludingAssessedTax",
    "SalesRevenueNet",
    "CostOfRevenue",
    "GrossProfit",
    "OperatingIncomeLoss",
    "NetIncomeLoss",
    "EarningsPerShareBasic",
    "EarningsPerShareDiluted",
    "Assets",
    "AssetsCurrent",
    "Liabilities",
    "LiabilitiesCurrent",
    "StockholdersEquity",
    "CashAndCashEquivalentsAtCarryingValue",
    "LongTermDebtNoncurrent",
    "NetCashProvidedByUsedInOperatingActivities",
    "PaymentsToAcquirePropertyPlantAndEquipment",
    "WeightedAverageNumberOfDilutedSharesOutstanding",
    "EntityCommonStockSharesOutstanding",
    "Revenue",
    "GrossProfitLoss",
    "ProfitLossFromOperatingActivities",
    "ProfitLoss",
    "BasicEarningsLossPerShare",
    "DilutedEarningsLossPerShare",
    "Equity",
    "CashAndCashEquivalents",
    "CashFlowsFromUsedInOperatingActivities",
    "PurchaseOfPropertyPlantAndEquipmentClassifiedAsInvestingActivities",
];
const CONCEPT_NAMESPACES: [&str; 3] = ["us-gaap", "ifrs-full", "dei"];

/// How a response becomes records, when no dot path can say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ResponseShape {
    /// `company_tickers.json`: one record per listed company.
    SecTickers,
    /// `submissions/CIK….json`: one record per result or event filing.
    EdgarFilings,
    /// `companyfacts/CIK….json`: one record per reported figure.
    XbrlFacts,
    /// A two-column `date,value` CSV whose second header names the series.
    DatedCsv,
    /// A JSON list of daily prices: raw close, volume, dividend, split.
    DailyPrices,
    /// SDMX CSV (ECB Data Portal `csvdata`, BIS `format=csv`): one row per
    /// date of one series. Without a `KEY` column (BIS), the series is the one
    /// queried.
    EcbCsv,
    /// filings.xbrl.org: the European annual and half-year reports of a
    /// company (ESEF), JSON:API with the entity included.
    EsefFilings,
    /// ALFRED `series/observations` in JSON, first releases only
    /// (`output_type=4`): each value dated by the day it was first published
    /// (D811). The series is the one queried.
    AlfredVintages,
    /// IMF PortWatch daily transits of a chokepoint (ArcGIS JSON), per day:
    /// vessels by kind and capacity (D885). The chokepoint is the one queried.
    PortwatchDaily,
    /// Open Prices (`prices.openfoodfacts.org`): one price of the queried
    /// product per shop and day; the shop is the metric, the currency the
    /// unit (pack `consumer_prices`, D883).
    OpenPrices,
    /// CFTC Commitments of Traders (legacy futures only, Socrata JSON): one
    /// weekly report of the queried market, positions by kind of trader.
    /// Positions are of Tuesday, published the Friday after: `known_on` is
    /// the report day plus three days.
    CftcCot,
    /// EIA API v2 `seriesid` (oil and gas statistics): `response.data` rows of
    /// `period` and `value`; the series is the one queried. A weekly series
    /// (the Weekly Petroleum Status Report) is known five days after the
    /// Friday its week ends, on the Wednesday it is published.
    EiaSeries,
}

/// A source that takes a key: fixed host, fixed vault name. A mapping names
/// the source, never a secret or a host: whoever may edit an import must not
/// be able to send a key elsewhere (R9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyedSource {
    pub key: &'static str,
    pub host: &'static str,
    /// Name of the org's vault secret.
    pub secret: &'static str,
    /// `Authorization` scheme, when the key goes in a header.
    pub scheme: &'static str,
    /// Query parameter, for a source that only takes its key in the URL
    /// (FRED): added at request time, never stored with the URL nor logged.
    pub query: Option<&'static str>,
}

pub const KEYED_SOURCES: [KeyedSource; 3] = [
    KeyedSource {
        key: "tiingo",
        host: "api.tiingo.com",
        secret: "markets_tiingo_key",
        scheme: "Token",
        query: None,
    },
    KeyedSource {
        key: "fred",
        host: "api.stlouisfed.org",
        secret: "markets_fred_key",
        scheme: "",
        query: Some("api_key"),
    },
    KeyedSource {
        key: "eia",
        host: "api.eia.gov",
        secret: "markets_eia_key",
        scheme: "",
        query: Some("api_key"),
    },
];

pub fn keyed_source(key: &str) -> Option<KeyedSource> {
    KEYED_SOURCES.iter().copied().find(|s| s.key == key)
}

/// The URL form of a code: an ECB series `FM.D.U2…` is `FM/D.U2…` in a path.
pub fn url_value(shape: Option<ResponseShape>, code: &str) -> String {
    match shape {
        Some(ResponseShape::EcbCsv) => code.replacen('.', "/", 1),
        // Setting codes are upper-cased; PortWatch ids are not.
        Some(ResponseShape::PortwatchDaily) => code.to_ascii_lowercase(),
        _ => code.to_string(),
    }
}

/// The records of a response, or `None` when it does not have this shape.
pub fn records(shape: ResponseShape, body: &[u8]) -> Option<Vec<Value>> {
    match shape {
        ResponseShape::DatedCsv => return dated_csv(std::str::from_utf8(body).ok()?),
        ResponseShape::EcbCsv => return ecb_csv(std::str::from_utf8(body).ok()?),
        _ => {}
    }
    let doc: Value = serde_json::from_slice(body).ok()?;
    match shape {
        ResponseShape::SecTickers => sec_tickers(&doc),
        ResponseShape::EdgarFilings => edgar_filings(&doc),
        ResponseShape::XbrlFacts => xbrl_facts(&doc),
        ResponseShape::DailyPrices => daily_prices(&doc),
        ResponseShape::EsefFilings => esef_filings(&doc),
        ResponseShape::AlfredVintages => alfred_vintages(&doc),
        ResponseShape::PortwatchDaily => portwatch_daily(&doc),
        ResponseShape::OpenPrices => open_prices(&doc),
        ResponseShape::CftcCot => cftc_cot(&doc),
        ResponseShape::EiaSeries => eia_series(&doc),
        ResponseShape::DatedCsv | ResponseShape::EcbCsv => None,
    }
}

/// A CIK as the SEC paths want it: ten digits.
fn cik10(v: &Value) -> Option<String> {
    let raw = match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().to_string(),
        _ => return None,
    };
    (!raw.is_empty() && raw.len() <= 10 && raw.bytes().all(|b| b.is_ascii_digit()))
        .then(|| format!("{raw:0>10}"))
}

fn is_date(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// The name the press uses: a register's legal suffix never appears in an
/// article's list of organizations, `NVIDIA CORP` is `Nvidia` there.
fn press_name(title: &str) -> String {
    const SUFFIXES: [&str; 14] = [
        "CORP",
        "CORPORATION",
        "INC",
        "INCORPORATED",
        "LTD",
        "LIMITED",
        "PLC",
        "CO",
        "COMPANY",
        "LLC",
        "LP",
        "NV",
        "SA",
        "AG",
    ];
    let mut words: Vec<&str> = title.split_whitespace().collect();
    while words.len() > 1 {
        let last = words[words.len() - 1].trim_matches(|c: char| !c.is_alphanumeric());
        if !SUFFIXES.contains(&last.to_ascii_uppercase().as_str()) {
            break;
        }
        words.pop();
    }
    words
        .join(" ")
        .trim_end_matches(|c: char| !c.is_alphanumeric())
        .to_string()
}

fn sec_tickers(doc: &Value) -> Option<Vec<Value>> {
    let rows = doc
        .as_object()?
        .values()
        .filter_map(|c| {
            Some(json!({
                "cik": cik10(c.get("cik_str")?)?,
                "ticker": c.get("ticker")?.as_str()?,
                "title": c.get("title")?.as_str()?,
                "press_name": press_name(c.get("title")?.as_str()?),
            }))
        })
        .collect::<Vec<_>>();
    (!rows.is_empty()).then_some(rows)
}

fn edgar_filings(doc: &Value) -> Option<Vec<Value>> {
    let cik = cik10(doc.get("cik")?)?;
    let recent = doc.get("filings")?.get("recent")?;
    let column = |name: &str| recent.get(name).and_then(Value::as_array);
    let (accessions, forms, filed) = (
        column("accessionNumber")?,
        column("form")?,
        column("filingDate")?,
    );
    let (accepted, reported, items, primary) = (
        column("acceptanceDateTime")?,
        column("reportDate")?,
        column("items")?,
        column("primaryDocument")?,
    );
    let text = |c: &[Value], i: usize| c.get(i).and_then(Value::as_str).unwrap_or("").to_string();
    // shortcut: `recent` only (the last 1000 filings or year); older ones sit
    // in `filings.files`, to read when a backtest needs a deeper history.
    let mut rows = Vec::new();
    for i in 0..accessions.len() {
        let (accession, form, filed_on, document) = (
            text(accessions, i),
            text(forms, i),
            text(filed, i),
            text(primary, i),
        );
        if !FORMS.contains(&form.as_str()) || !is_date(&filed_on) || accession.is_empty() {
            continue;
        }
        // The path segments come from the answer: only a plain name goes in.
        let plain = !document.is_empty()
            && document
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
        let digits = accession.replace('-', "");
        let url = (plain && digits.bytes().all(|b| b.is_ascii_digit())).then(|| {
            format!(
                "https://www.sec.gov/Archives/edgar/data/{}/{digits}/{document}",
                cik.trim_start_matches('0')
            )
        });
        rows.push(json!({
            "cik": cik,
            "accession": accession,
            "form": form,
            "filed_on": filed_on,
            "accepted_at": text(accepted, i),
            "report_on": text(reported, i),
            "items": text(items, i),
            "url": url,
        }));
    }
    Some(rows)
}

fn xbrl_facts(doc: &Value) -> Option<Vec<Value>> {
    let facts = doc.get("facts")?.as_object()?;
    let mut rows = Vec::new();
    for ns in CONCEPT_NAMESPACES {
        let Some(concepts) = facts.get(ns).and_then(Value::as_object) else {
            continue;
        };
        for name in CONCEPTS {
            let Some(units) = concepts
                .get(name)
                .and_then(|c| c.get("units"))
                .and_then(Value::as_object)
            else {
                continue;
            };
            for (unit, entries) in units {
                for e in entries.as_array().into_iter().flatten() {
                    let (Some(end), Some(filed), Some(value)) = (
                        e.get("end").and_then(Value::as_str),
                        e.get("filed").and_then(Value::as_str),
                        e.get("val").and_then(Value::as_f64),
                    ) else {
                        continue;
                    };
                    rows.push(json!({
                        "metric": name,
                        "unit": unit,
                        "start": e.get("start").and_then(Value::as_str),
                        "end": end,
                        "value": value,
                        "known_on": filed,
                        "source": e.get("accn").and_then(Value::as_str),
                    }));
                }
            }
        }
    }
    Some(rows)
}

fn dated_csv(text: &str) -> Option<Vec<Value>> {
    let table = parse_csv(text, Some(','));
    let header = table.first()?;
    let [dates, series] = header.as_slice() else {
        return None;
    };
    let (dates, series) = (dates.trim().to_string(), series.trim().to_string());
    if dates.is_empty() || series.is_empty() || dates == series {
        return None;
    }
    let mut rows = Vec::new();
    for r in table_rows(table) {
        let date = r.get(&dates)?.as_str()?.trim().to_string();
        if !is_date(&date) {
            return None;
        }
        // FRED writes a missing value as an empty cell or a dot.
        let raw = r.get(&series).and_then(Value::as_str).unwrap_or("").trim();
        if let Some(value) = raw.parse::<f64>().ok().filter(|v| v.is_finite()) {
            rows.push(json!({ "metric": series, "end": date, "value": value }));
        }
    }
    Some(rows)
}

fn alfred_vintages(doc: &Value) -> Option<Vec<Value>> {
    let mut rows = Vec::new();
    for o in doc.get("observations")?.as_array()? {
        let date = o.get("date")?.as_str()?;
        let known = o.get("realtime_start")?.as_str()?;
        if !is_date(date) || !is_date(known) {
            return None;
        }
        // A missing value is a dot, as in the CSV.
        let raw = o.get("value").and_then(Value::as_str).unwrap_or("").trim();
        if let Some(value) = raw.parse::<f64>().ok().filter(|v| v.is_finite()) {
            rows.push(json!({ "end": date, "value": value, "known_on": known }));
        }
    }
    Some(rows)
}

fn daily_prices(doc: &Value) -> Option<Vec<Value>> {
    let mut rows = Vec::new();
    for bar in doc.as_array()? {
        let date = bar.get("date")?.as_str()?.get(..10)?;
        if !is_date(date) {
            return None;
        }
        let number = |k: &str| bar.get(k).and_then(Value::as_f64).filter(|v| v.is_finite());
        let mut push = |metric: &str, value: f64| {
            // Known at the end of its own day, like a filing.
            rows.push(json!({ "metric": metric, "end": date, "value": value, "known_on": date }));
        };
        push("close", number("close")?);
        if let Some(v) = number("volume") {
            push("volume", v);
        }
        if let Some(v) = number("divCash").filter(|v| *v != 0.0) {
            push("dividend", v);
        }
        if let Some(v) = number("splitFactor").filter(|v| *v != 1.0) {
            push("split", v);
        }
    }
    Some(rows)
}

/// `2026-09` or `2026-Q3` as the first day of the period.
fn period_start(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if is_date(raw) {
        return Some(raw.to_string());
    }
    // An annual period (EIA): its first day.
    if raw.len() == 4 && raw.bytes().all(|b| b.is_ascii_digit()) {
        return Some(format!("{raw}-01-01"));
    }
    let (year, rest) = raw.split_once('-')?;
    let month = match rest.strip_prefix('Q') {
        Some(q) => format!(
            "{:02}",
            q.parse::<u32>().ok().filter(|q| (1..=4).contains(q))? * 3 - 2
        ),
        None => rest.to_string(),
    };
    let date = format!("{year}-{month}-01");
    is_date(&date).then_some(date)
}

fn ecb_csv(text: &str) -> Option<Vec<Value>> {
    let table = parse_csv(text, Some(','));
    let header = table.first()?;
    if !["TIME_PERIOD", "OBS_VALUE"]
        .iter()
        .all(|h| header.iter().any(|c| c.trim() == *h))
    {
        return None;
    }
    let mut rows = Vec::new();
    for r in table_rows(table) {
        let text = |k: &str| {
            r.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let end = period_start(&text("TIME_PERIOD"))?;
        if let Some(value) = text("OBS_VALUE")
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
        {
            let mut row = json!({ "end": end, "value": value });
            if !text("KEY").is_empty() {
                row["metric"] = json!(text("KEY"));
            }
            rows.push(row);
        }
    }
    Some(rows)
}

/// Measures of a PortWatch day kept as series metrics.
const PORTWATCH_METRICS: [&str; 6] = [
    "n_total",
    "n_tanker",
    "n_container",
    "n_dry_bulk",
    "n_cargo",
    "capacity",
];

/// A shop as a series name: readable, and unique by its OpenStreetMap id.
fn shop_metric(item: &Value) -> String {
    let loc = item.get("location");
    let text = |k: &str| {
        loc.and_then(|l| l.get(k))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
    };
    let id = item
        .get("location_osm_id")
        .and_then(Value::as_i64)
        .map_or_else(|| "online".to_string(), |i| i.to_string());
    let place = [text("osm_address_city"), text("osm_address_country_code")]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let mut m = match text("osm_name") {
        Some(n) if !place.is_empty() => format!("{n}, {place} ({id})"),
        Some(n) => format!("{n} ({id})"),
        None => format!("shop {id}"),
    };
    if item.get("price_is_discounted").and_then(Value::as_bool) == Some(true) {
        m.push_str(" [discount]");
    }
    m
}

fn open_prices(doc: &Value) -> Option<Vec<Value>> {
    let mut rows = Vec::new();
    for item in doc.get("items")?.as_array()? {
        let Some(date) = item
            .get("date")
            .and_then(Value::as_str)
            .filter(|d| is_date(d))
        else {
            continue;
        };
        let (Some(price), Some(currency)) = (
            item.get("price")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v > 0.0),
            item.get("currency").and_then(Value::as_str),
        ) else {
            continue;
        };
        let name = item
            .get("product")
            .and_then(|p| p.get("product_name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        rows.push(json!({
            "metric": shop_metric(item), "unit": currency, "end": date,
            "known_on": date, "value": price, "name": name,
        }));
    }
    Some(rows)
}

/// Days between the end of an EIA petroleum week (Friday) and its report.
// shortcut: a report delayed by a holiday keeps the five days; read the EIA release schedule if a past-dated test shows it matters.
const EIA_WEEKLY_RELEASE_DAYS: u64 = 5;

fn eia_series(doc: &Value) -> Option<Vec<Value>> {
    let resp = doc.get("response")?;
    let weekly = resp
        .get("frequency")
        .and_then(Value::as_str)
        .is_some_and(|f| f.eq_ignore_ascii_case("weekly"));
    let mut rows = Vec::new();
    for r in resp.get("data")?.as_array()? {
        let Some(end) = r
            .get("period")
            .and_then(Value::as_str)
            .and_then(period_start)
        else {
            continue;
        };
        let value = match r.get("value") {
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
            _ => None,
        };
        let Some(value) = value.filter(|v| v.is_finite()) else {
            continue;
        };
        let mut row = json!({ "end": end, "value": value });
        if let Some(u) = r.get("units").and_then(Value::as_str) {
            row["unit"] = json!(u);
        }
        if weekly {
            let known = chrono::NaiveDate::parse_from_str(&end, "%Y-%m-%d")
                .ok()?
                .checked_add_days(chrono::Days::new(EIA_WEEKLY_RELEASE_DAYS))?;
            row["known_on"] = json!(known.to_string());
        }
        rows.push(row);
    }
    Some(rows)
}

/// Days between the positions of a COT report (Tuesday) and its release.
// shortcut: a release delayed by a holiday or a shutdown is dated three days after anyway; read the CFTC release calendar if a past-dated test shows it matters.
const COT_RELEASE_DAYS: u64 = 3;

fn cftc_cot(doc: &Value) -> Option<Vec<Value>> {
    let mut rows = Vec::new();
    for r in doc.as_array()? {
        let Some(day) = r
            .get("report_date_as_yyyy_mm_dd")
            .and_then(Value::as_str)
            .and_then(|d| d.get(..10))
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        else {
            continue;
        };
        let known = day.checked_add_days(chrono::Days::new(COT_RELEASE_DAYS))?;
        let n = |k: &str| {
            r.get(k)
                .and_then(Value::as_str)
                .and_then(|v| v.trim().parse::<f64>().ok())
        };
        let name = r
            .get("market_and_exchange_names")
            .and_then(Value::as_str)
            .unwrap_or("");
        let (end, known) = (day.to_string(), known.to_string());
        let mut push = |metric: &str, v: Option<f64>| {
            if let Some(value) = v {
                rows.push(json!({ "metric": metric, "end": end, "known_on": known, "value": value, "name": name }));
            }
        };
        let (nl, ns) = (
            n("noncomm_positions_long_all"),
            n("noncomm_positions_short_all"),
        );
        let (cl, cs) = (n("comm_positions_long_all"), n("comm_positions_short_all"));
        push("open_interest", n("open_interest_all"));
        push("noncommercial_long", nl);
        push("noncommercial_short", ns);
        push("noncommercial_net", nl.zip(ns).map(|(l, s)| l - s));
        push("commercial_long", cl);
        push("commercial_short", cs);
        push("commercial_net", cl.zip(cs).map(|(l, s)| l - s));
        push("nonreportable_long", n("nonrept_positions_long_all"));
        push("nonreportable_short", n("nonrept_positions_short_all"));
    }
    Some(rows)
}

fn portwatch_daily(doc: &Value) -> Option<Vec<Value>> {
    let mut rows = Vec::new();
    for f in doc.get("features")?.as_array()? {
        let a = f.get("attributes")?;
        // A date-only field is `YYYY-MM-DD`; an older layer gives epoch ms.
        let date = match a.get("date")? {
            Value::String(s) => s.get(..10)?.to_string(),
            Value::Number(n) => chrono::DateTime::from_timestamp_millis(n.as_i64()?)?
                .date_naive()
                .to_string(),
            _ => return None,
        };
        if !is_date(&date) {
            return None;
        }
        let name = a.get("portname").and_then(Value::as_str).unwrap_or("");
        for m in PORTWATCH_METRICS {
            if let Some(v) = a.get(m).and_then(Value::as_f64).filter(|v| v.is_finite()) {
                rows.push(json!({ "metric": m, "end": date, "value": v, "name": name }));
            }
        }
    }
    Some(rows)
}

fn esef_filings(doc: &Value) -> Option<Vec<Value>> {
    let entity = doc
        .get("included")
        .and_then(Value::as_array)
        .and_then(|i| {
            i.iter()
                .find(|e| e.get("type").and_then(Value::as_str) == Some("entity"))
        })
        .and_then(|e| e.get("attributes"));
    let name = entity.and_then(|e| e.get("name")).and_then(Value::as_str);
    let mut rows = Vec::new();
    for f in doc.get("data")?.as_array()? {
        let a = f.get("attributes")?;
        let text = |k: &str| a.get(k).and_then(Value::as_str);
        let Some(id) = text("fxo_id") else { continue };
        // An old report may lack its publication day: the day it was added
        // is later, never earlier, so a dated reading stays honest.
        let Some(filed) = text("published")
            .or_else(|| text("date_added"))
            .and_then(|d| d.get(..10))
            .filter(|d| is_date(d))
        else {
            continue;
        };
        let url = text("report_url")
            .filter(|p| p.starts_with('/') && !p.contains(".."))
            .map(|p| format!("{ESEF_BASE}{p}"));
        rows.push(json!({
            "accession": id.chars().take(64).collect::<String>(),
            "form": "ESEF",
            "filed_on": filed,
            "report_on": text("period_end"),
            "url": url,
            "name": name,
        }));
    }
    Some(rows)
}

/// Where the documents of the filings may be read from.
pub const ESEF_BASE: &str = "https://filings.xbrl.org";
pub const SEC_ARCHIVE: &str = "https://www.sec.gov/Archives/edgar/data/";

/// Where a mapping's observations belong: the live object of this type whose
/// unique property holds the value the record was queried with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ObservationTarget {
    #[serde(rename = "type")]
    pub type_key: String,
    pub property: String,
}

/// One value of a dated metric, as a source gave it (D541).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PlannedObservation {
    pub key: String,
    pub metric: String,
    /// Empty when the source gives none.
    #[serde(default)]
    pub unit: String,
    /// Equal to `period_end` for a value at a date.
    pub period_start: String,
    pub period_end: String,
    /// The day the source says it published this value; `None` = unknown,
    /// the value counts as known from the moment it is stored.
    #[serde(default)]
    pub known_on: Option<String>,
    pub value: f64,
    #[serde(default)]
    pub source: Option<String>,
}

/// The observations of a run's records. A later filing repeating a value
/// unchanged says nothing new: only the first report and real restatements
/// remain, which is what a reading "as known at a date" needs.
pub fn observations(rows: &[Value]) -> Vec<PlannedObservation> {
    first_reports(rows.iter().filter_map(observation).collect())
}

/// One record read as an observation: columns `@queried`, `metric`, `unit`,
/// `start`, `end`, `known_on`, `value`, `source`. A table gives every value as
/// text: `value` may be a number or its text.
pub fn observation(r: &Value) -> Option<PlannedObservation> {
    let text = |k: &str| r.get(k).and_then(Value::as_str).map(str::to_string);
    let end = text("end").filter(|d| is_date(d))?;
    let value = match r.get("value")? {
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        v => v.as_f64()?,
    };
    Some(PlannedObservation {
        key: text(QUERIED)?,
        metric: text("metric").filter(|m| !m.is_empty())?,
        unit: text("unit").unwrap_or_default(),
        period_start: text("start").filter(|d| is_date(d)).unwrap_or(end.clone()),
        period_end: end,
        known_on: text("known_on").filter(|d| is_date(d)),
        value: Some(value).filter(|v| v.is_finite())?,
        source: text("source"),
    })
}

/// Keeps the first report of each value and its real restatements.
pub fn first_reports(mut out: Vec<PlannedObservation>) -> Vec<PlannedObservation> {
    let same_period = |a: &PlannedObservation, b: &PlannedObservation| {
        (&a.key, &a.metric, &a.unit, &a.period_start, &a.period_end)
            == (&b.key, &b.metric, &b.unit, &b.period_start, &b.period_end)
    };
    out.sort_by(|a, b| {
        (
            &a.key,
            &a.metric,
            &a.unit,
            &a.period_start,
            &a.period_end,
            &a.known_on,
        )
            .cmp(&(
                &b.key,
                &b.metric,
                &b.unit,
                &b.period_start,
                &b.period_end,
                &b.known_on,
            ))
    });
    let mut kept: Vec<PlannedObservation> = Vec::with_capacity(out.len());
    for o in out {
        match kept.last() {
            Some(p) if same_period(p, &o) && p.value == o.value => {}
            _ => kept.push(o),
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tickers_pad_the_cik() {
        let body = br#"{"0":{"cik_str":42,"ticker":"ACME","title":"ACME CORP"}}"#;
        let r = records(ResponseShape::SecTickers, body).unwrap();
        assert_eq!(r[0]["cik"], "0000000042");
        assert_eq!(r[0]["ticker"], "ACME");
        assert!(records(ResponseShape::SecTickers, b"{}").is_none());
    }

    #[test]
    fn the_press_name_loses_the_legal_suffix_only() {
        assert_eq!(press_name("NVIDIA CORP"), "NVIDIA");
        assert_eq!(press_name("Meta Platforms, Inc."), "Meta Platforms");
        assert_eq!(press_name("Coca Cola Co"), "Coca Cola");
        assert_eq!(press_name("Inc"), "Inc", "a name is never emptied");
    }

    #[test]
    fn filings_keep_result_forms_and_build_the_document_url() {
        let body = json!({
            "cik": "42",
            "filings": {"recent": {
                "accessionNumber": ["0000000042-26-000001", "0000000042-26-000002", "0000000042-26-000003"],
                "filingDate": ["2026-08-26", "2026-08-27", "2026-08-28"],
                "reportDate": ["2026-07-26", "", ""],
                "acceptanceDateTime": ["2026-08-26T20:15:00.000Z", "", ""],
                "form": ["10-Q", "4", "8-K"],
                "items": ["", "", "2.02"],
                "primaryDocument": ["acme-20260726.htm", "x.xml", "../../evil.htm"]
            }}
        })
        .to_string();
        let r = records(ResponseShape::EdgarFilings, body.as_bytes()).unwrap();
        assert_eq!(r.len(), 2, "the insider form stays out");
        assert_eq!(
            r[0]["url"],
            "https://www.sec.gov/Archives/edgar/data/42/000000004226000001/acme-20260726.htm"
        );
        assert_eq!(r[1]["url"], Value::Null, "a path from the answer stays out");
    }

    #[test]
    fn facts_keep_known_concepts_with_their_filing_day() {
        let body = json!({"facts": {
            "us-gaap": {
                "Assets": {"units": {"USD": [{"end": "2025-12-31", "val": 10, "filed": "2026-02-01", "accn": "a1"}]}},
                "SomethingRare": {"units": {"USD": [{"end": "2025-12-31", "val": 1, "filed": "2026-02-01"}]}}
            }
        }})
        .to_string();
        let r = records(ResponseShape::XbrlFacts, body.as_bytes()).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["metric"], "Assets");
        assert_eq!(r[0]["known_on"], "2026-02-01");
    }

    #[test]
    fn alfred_first_releases_are_dated_by_their_publication() {
        let body = br#"{"observations": [
            {"realtime_start": "2026-02-13", "realtime_end": "2026-03-12", "date": "2026-01-01", "value": "3.1"},
            {"realtime_start": "2026-03-13", "realtime_end": "9999-12-31", "date": "2026-02-01", "value": "."}
        ]}"#;
        let r = records(ResponseShape::AlfredVintages, body).unwrap();
        assert_eq!(r.len(), 1, "the dot is a missing value");
        assert_eq!(r[0]["known_on"], "2026-02-13");
        assert_eq!(r[0]["end"], "2026-01-01");
        assert!(records(ResponseShape::AlfredVintages, br#"{"error_code": 400}"#).is_none());
    }

    #[test]
    fn dated_csv_names_the_series_and_skips_missing_values() {
        let body = b"observation_date,DGS10\n2026-01-02,4.19\n2026-01-03,\n2026-01-04,.\n";
        let r = records(ResponseShape::DatedCsv, body).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["metric"], "DGS10");
        assert_eq!(r[0]["value"], 4.19);
        // An error page is not a series.
        assert!(records(ResponseShape::DatedCsv, b"<html>\n<body>no</body>\n").is_none());
        assert!(records(ResponseShape::DatedCsv, b"a,b\nnot-a-date,1\n").is_none());
    }

    #[test]
    fn prices_keep_the_raw_close_and_only_real_corporate_actions() {
        let body = json!([
            {"date": "2026-10-30T00:00:00.000Z", "close": 10.0, "volume": 5, "divCash": 0.0, "splitFactor": 1.0, "adjClose": 9.0},
            {"date": "2026-10-31T00:00:00.000Z", "close": 1.0, "volume": 7, "divCash": 0.5, "splitFactor": 10.0}
        ])
        .to_string();
        let r = records(ResponseShape::DailyPrices, body.as_bytes()).unwrap();
        let metrics: Vec<&str> = r.iter().map(|x| x["metric"].as_str().unwrap()).collect();
        assert_eq!(
            metrics,
            ["close", "volume", "close", "volume", "dividend", "split"]
        );
        assert_eq!(r[0]["known_on"], "2026-10-30");
        assert_eq!(r[0]["value"], 10.0, "never the adjusted close");
        assert!(records(ResponseShape::DailyPrices, b"{\"detail\":\"no\"}").is_none());
    }

    #[test]
    fn eia_weekly_stocks_are_known_the_wednesday_after() {
        let body = br#"{"response":{"frequency":"weekly","data":[
            {"period":"2026-10-30","value":"421345","units":"MBBL"},
            {"period":"2026-10-23","value":null}]}}"#;
        let r = records(ResponseShape::EiaSeries, body).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["known_on"], "2026-11-04");
        assert_eq!(r[0]["unit"], "MBBL");
        let monthly =
            br#"{"response":{"frequency":"monthly","data":[{"period":"2026-08","value":13.4}]}}"#;
        let r = records(ResponseShape::EiaSeries, monthly).unwrap();
        assert_eq!(r[0]["end"], "2026-08-01");
        assert!(
            r[0].get("known_on").is_none(),
            "unknown release day: left to the reading"
        );
        assert_eq!(keyed_source("eia").unwrap().host, "api.eia.gov");
    }

    #[test]
    fn cot_reports_are_known_the_friday_after() {
        let body = br#"[{"report_date_as_yyyy_mm_dd":"2026-11-03T00:00:00.000","market_and_exchange_names":"GOLD - COMMODITY EXCHANGE INC.",
            "open_interest_all":"399169","noncomm_positions_long_all":"220530","noncomm_positions_short_all":"40982",
            "comm_positions_long_all":"64326","comm_positions_short_all":"269228",
            "nonrept_positions_long_all":"46024","nonrept_positions_short_all":"20670"},
            {"report_date_as_yyyy_mm_dd":"soon"}]"#;
        let r = records(ResponseShape::CftcCot, body).unwrap();
        assert_eq!(r.len(), 9);
        let net = r
            .iter()
            .find(|x| x["metric"] == "noncommercial_net")
            .unwrap();
        assert_eq!(net["value"], 179548.0);
        assert_eq!(net["end"], "2026-11-03");
        assert_eq!(
            net["known_on"], "2026-11-06",
            "a Tuesday report, released on Friday"
        );
        assert_eq!(net["name"], "GOLD - COMMODITY EXCHANGE INC.");
        assert!(records(ResponseShape::CftcCot, b"{}").is_none());
    }

    #[test]
    fn open_prices_become_one_series_per_shop() {
        let body = br#"{"items":[
            {"price":4.05,"currency":"EUR","date":"2026-11-05","location_osm_id":2111106074,"price_is_discounted":false,
             "product":{"product_name":"Nutella"},"location":{"osm_name":"Carrefour City","osm_address_city":"Nancy","osm_address_country_code":"FR"}},
            {"price":3.5,"currency":"EUR","date":"2026-11-04","location_osm_id":42,"price_is_discounted":true,"location":{}},
            {"price":0,"currency":"EUR","date":"2026-11-04"},
            {"price":2,"currency":"EUR","date":"soon"}
        ]}"#;
        let r = records(ResponseShape::OpenPrices, body).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["metric"], "Carrefour City, Nancy FR (2111106074)");
        assert_eq!(r[0]["unit"], "EUR");
        assert_eq!(r[0]["name"], "Nutella");
        assert_eq!(r[1]["metric"], "shop 42 [discount]");
        assert!(records(ResponseShape::OpenPrices, b"[]").is_none());
    }

    #[test]
    fn portwatch_days_become_one_value_per_measure() {
        let body = br#"{"features":[{"attributes":{"date":"2026-11-01","portname":"Suez Canal","n_total":46,"n_tanker":9,"capacity":2080955}},{"attributes":{"date":1761955200000,"portname":"Suez Canal","n_total":44}}]}"#;
        let r = records(ResponseShape::PortwatchDaily, body).unwrap();
        assert_eq!(r.len(), 4);
        assert_eq!(r[0]["metric"], "n_total");
        assert_eq!(r[0]["name"], "Suez Canal");
        assert_eq!(r[3]["end"], "2025-11-01");
        assert_eq!(
            url_value(Some(ResponseShape::PortwatchDaily), "CHOKEPOINT6"),
            "chokepoint6"
        );
        assert!(records(ResponseShape::PortwatchDaily, b"{}").is_none());
    }

    #[test]
    fn ecb_periods_become_days_and_the_code_a_path() {
        let body = b"KEY,TIME_PERIOD,OBS_VALUE\nICP.M.U2,2026-09,2.1\nICP.M.U2,2026-Q3,2.2\nICP.M.U2,2026-10-02,\n";
        let r = records(ResponseShape::EcbCsv, body).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["end"], "2026-09-01");
        assert_eq!(r[1]["end"], "2026-07-01");
        assert!(records(ResponseShape::EcbCsv, b"a,b\n1,2\n").is_none());
        // BIS: no KEY column, the source fills in the queried series.
        let bis =
            b"FREQ,REF_AREA,TITLE,TIME_PERIOD,OBS_VALUE\nD,US, Policy rate,2026-10-01,3.875\n";
        let r = records(ResponseShape::EcbCsv, bis).unwrap();
        assert_eq!(r, vec![json!({"end": "2026-10-01", "value": 3.875})]);
        assert_eq!(
            url_value(Some(ResponseShape::EcbCsv), "FM.D.U2.EUR"),
            "FM/D.U2.EUR"
        );
        assert_eq!(url_value(Some(ResponseShape::DatedCsv), "A.B"), "A.B");
    }

    #[test]
    fn esef_reports_are_dated_and_their_path_checked() {
        let body = json!({
            "data": [
                {"attributes": {"fxo_id": "LEI-1", "published": "2026-10-26 15:35:33", "date_added": "2026-10-28 12:00:00",
                    "period_end": "2026-06-30", "report_url": "/x/report.xhtml"}},
                {"attributes": {"fxo_id": "LEI-2", "published": null, "date_added": "2023-01-02 00:00:00",
                    "report_url": "/../evil"}}
            ],
            "included": [{"type": "entity", "attributes": {"name": "ACME SA"}}]
        })
        .to_string();
        let r = records(ResponseShape::EsefFilings, body.as_bytes()).unwrap();
        assert_eq!(r[0]["filed_on"], "2026-10-26");
        assert_eq!(r[0]["url"], "https://filings.xbrl.org/x/report.xhtml");
        assert_eq!(r[0]["name"], "ACME SA");
        assert_eq!(r[1]["filed_on"], "2023-01-02", "never earlier than known");
        assert_eq!(r[1]["url"], Value::Null);
    }

    #[test]
    fn a_key_only_goes_to_its_host() {
        let s = keyed_source("tiingo").unwrap();
        assert_eq!(s.host, "api.tiingo.com");
        assert!(keyed_source("other").is_none());
    }

    fn row(end: &str, filed: &str, value: f64) -> Value {
        json!({"@queried": "k", "metric": "Assets", "unit": "USD", "end": end,
               "value": value, "known_on": filed})
    }

    #[test]
    fn a_repeated_value_is_dropped_and_a_restatement_kept() {
        let got = observations(&[
            row("2025-12-31", "2027-02-01", 10.0),
            row("2025-12-31", "2026-02-01", 10.0),
            row("2025-12-31", "2026-05-01", 12.0),
        ]);
        let seen: Vec<(Option<&str>, f64)> = got
            .iter()
            .map(|o| (o.known_on.as_deref(), o.value))
            .collect();
        assert_eq!(
            seen,
            vec![
                (Some("2026-02-01"), 10.0),
                (Some("2026-05-01"), 12.0),
                (Some("2027-02-01"), 10.0)
            ],
            "the value that came back after a restatement is a change too"
        );
        assert_eq!(got[0].period_start, "2025-12-31", "a value at a date");
    }

    #[test]
    fn a_record_without_a_usable_date_or_value_is_no_observation() {
        let bad = [
            json!({"@queried": "k", "metric": "m", "end": "soon", "value": 1.0}),
            json!({"@queried": "k", "metric": "m", "end": "2026-01-01"}),
            json!({"metric": "m", "end": "2026-01-01", "value": 1.0}),
        ];
        assert!(observations(&bad).is_empty());
    }
}
