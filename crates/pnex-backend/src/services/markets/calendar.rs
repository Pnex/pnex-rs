//! Scheduled releases and meetings (markets-research.md D882): each day, the
//! calendars an org lists (`markets.calendars`) are read from the
//! institutions themselves and written as time ranges of the org (D182),
//! keyed by `external_id`: the calendar page and the assistant's calendar
//! tools show them, and a moved date updates its range.

use chrono::{Duration, Utc};
use loco_rs::app::AppContext;
use pnex_core::markets::calendar::{self as cal, Scheduled};
use pnex_core::time_range::TimeRangeInput;
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};

use crate::services::ontology::imports::source;
use crate::services::time_ranges::{self, import};

pub const BLS_URL: &str = "https://www.bls.gov/schedule/news_release/bls.ics";
pub const FOMC_URL: &str = "https://www.federalreserve.gov/monetarypolicy/fomccalendars.htm";
pub const ECB_URL: &str = "https://www.ecb.europa.eu/press/calendars/mgcgc/html/index.en.html";

/// A calendar the setting may name: its key, the prefix of its ranges'
/// `external_id`, its category on the calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Calendar {
    Bls,
    Fomc,
    Ecb,
}

impl Calendar {
    const ALL: [Calendar; 3] = [Self::Bls, Self::Fomc, Self::Ecb];

    fn key(self) -> &'static str {
        match self {
            Self::Bls => "bls",
            Self::Fomc => "fomc",
            Self::Ecb => "ecb",
        }
    }

    fn category(self) -> &'static str {
        match self {
            Self::Bls => "economic_release",
            Self::Fomc | Self::Ecb => "central_bank_meeting",
        }
    }
}

/// The calendars of the setting, known keys only.
fn listed(raw: &str) -> Vec<Calendar> {
    Calendar::ALL
        .into_iter()
        .filter(|c| {
            raw.split(',')
                .any(|k| k.trim().eq_ignore_ascii_case(c.key()))
        })
        .collect()
}

/// The URLs read, the institutions' own unless tests point them at a mock.
#[derive(Debug, Clone)]
pub struct Urls {
    pub bls: String,
    pub fomc: String,
    pub ecb: String,
}

impl Default for Urls {
    fn default() -> Self {
        Self {
            bls: BLS_URL.into(),
            fomc: FOMC_URL.into(),
            ecb: ECB_URL.into(),
        }
    }
}

impl Urls {
    fn of(&self, c: Calendar) -> &str {
        match c {
            Calendar::Bls => &self.bls,
            Calendar::Fomc => &self.fomc,
            Calendar::Ecb => &self.ecb,
        }
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Read today already: one read a day per org and calendar.
async fn fresh<C: ConnectionTrait>(db: &C, org: i64, c: Calendar) -> Result<bool, DbErr> {
    let row = db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM time_ranges WHERE org_id = $1 AND scope_kind = 'org' \
               AND external_id LIKE $2 AND updated_at > $3 LIMIT 1",
            vec![
                org.into(),
                format!("{}:%", c.key()).into(),
                (Utc::now() - Duration::hours(20)).into(),
            ],
        ))
        .await?;
    Ok(row.is_some())
}

/// The ranges of a calendar's answer, `None` when it is not readable.
fn rows_of(c: Calendar, url: &str, body: &[u8]) -> Option<Vec<import::Row>> {
    let mut rows = match c {
        Calendar::Bls => import::parse(import::Format::Ics, body).ok()?,
        Calendar::Fomc | Calendar::Ecb => {
            let html = String::from_utf8_lossy(body);
            let dates = match c {
                Calendar::Fomc => cal::fomc_calendar(&html)?,
                _ => cal::ecb_calendar(&html)?,
            };
            dates.into_iter().map(day_row).collect()
        }
    };
    for r in &mut rows {
        let i = &mut r.input;
        // A UID is the BLS's own; the meeting pages' ids carry their prefix.
        if c == Calendar::Bls {
            i.external_id = i.external_id.as_deref().map(|e| format!("bls:{e}"));
            // A release is an instant (`DURATION:PT0M`): a range needs an
            // end after its start, so it lasts one minute.
            let at = |t: &Option<String>| {
                t.as_deref()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            };
            if let Some(start) = at(&i.planned_start) {
                if at(&i.planned_end).is_none_or(|end| end <= start) {
                    i.planned_end = Some((start + Duration::minutes(1)).to_rfc3339());
                }
            }
        }
        i.category = Some(c.category().into());
        i.source_url = Some(url.to_string());
    }
    Some(rows)
}

/// A meeting known by its day only: the range covers that day, in UTC.
fn day_row(s: Scheduled) -> import::Row {
    let at = |d: chrono::NaiveDate| d.and_hms_opt(0, 0, 0).map(|t| t.and_utc().to_rfc3339());
    import::Row {
        line: 0,
        input: TimeRangeInput {
            label: Some(s.label),
            external_id: Some(s.external_id),
            planned_start: at(s.day),
            planned_end: s.day.succ_opt().and_then(at),
            ..Default::default()
        },
    }
}

/// One pass: every org with the markets pack, each calendar it lists and
/// has not read today, with the org's declared contact (the BLS refuses a
/// client without one).
pub async fn sweep(ctx: &AppContext, urls: &Urls) -> Result<(), DbErr> {
    let orgs: Vec<i64> = ctx
        .db
        .query_all_raw(stmt(
            "SELECT org_id FROM packs WHERE key = 'markets' ORDER BY org_id",
            vec![],
        ))
        .await?
        .iter()
        .filter_map(|r| r.try_get("", "org_id").ok())
        .collect();
    for org in orgs {
        let text = |key: &'static str| async move {
            crate::services::ontology::packs::setting_value(&ctx.db, org, key)
                .await
                .ok()
                .flatten()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        };
        let calendars = listed(&text("markets.calendars").await);
        if calendars.is_empty() {
            continue;
        }
        let contact = text("markets.source_contact").await;
        let by = source::Declared {
            contact: Some(contact).filter(|c| !c.trim().is_empty()),
            ..Default::default()
        };
        let scope = match time_ranges::resolve_scope(&ctx.db, org, "org", "").await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(org, error = %e, "market calendars: no org scope");
                continue;
            }
        };
        for c in calendars {
            if fresh(&ctx.db, org, c).await? {
                continue;
            }
            let url = urls.of(c);
            let Ok(u) = source::check_url(url) else {
                continue;
            };
            let body = match source::get_found(&u, &by).await {
                Ok(Some(b)) => b,
                Ok(None) | Err(_) => {
                    tracing::info!(org, calendar = c.key(), "market calendar not read");
                    continue;
                }
            };
            let Some(rows) = rows_of(c, url, &body) else {
                tracing::warn!(org, calendar = c.key(), "market calendar unreadable");
                continue;
            };
            let r = import::run(&ctx.db, org, &scope, rows).await?;
            tracing::info!(
                org,
                calendar = c.key(),
                created = r.created,
                updated = r.updated,
                rejected = r.rejected,
                "market calendar read"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_names_known_calendars_only() {
        assert_eq!(listed(" BLS, ecb ,x"), vec![Calendar::Bls, Calendar::Ecb]);
        assert!(listed("").is_empty());
    }

    #[test]
    fn each_range_is_keyed_by_its_calendar() {
        let ics = b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u-1\r\nDTSTART;TZID=US-Eastern:20261210T083000\r\nSUMMARY:Consumer Price Index\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let rows = rows_of(Calendar::Bls, BLS_URL, ics).unwrap();
        let i = &rows[0].input;
        assert_eq!(i.external_id.as_deref(), Some("bls:u-1"));
        assert_eq!(i.category.as_deref(), Some("economic_release"));
        // The BLS zone name is read as New York time.
        assert_eq!(
            i.planned_start.as_deref(),
            Some("2026-12-10T08:30:00-05:00")
        );
        let html = b"Governing Council <dt>17/12/2026</dt><dd>Governing Council of the ECB: monetary policy meeting (Day 2)</dd>";
        let rows = rows_of(Calendar::Ecb, ECB_URL, html).unwrap();
        assert_eq!(rows[0].input.external_id.as_deref(), Some("ecb:2026-12-17"));
        assert_eq!(
            rows[0].input.planned_start.as_deref(),
            Some("2026-12-17T00:00:00+00:00")
        );
        assert!(rows_of(Calendar::Fomc, FOMC_URL, b"<html>").is_none());
    }
}
