//! Scheduled meetings (markets-research.md D882): the dates a central bank
//! publishes ahead, read from its own page. They become time ranges of the
//! org (D182), like an iCalendar release schedule read by
//! `time_ranges::import`. Pure code: no I/O, no clock; a page that no
//! longer has its known form gives `None`.

use chrono::NaiveDate;

/// One date an institution announced: `external_id` stays the same when
/// the page is read again, so a moved meeting updates its range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scheduled {
    pub external_id: String,
    pub label: String,
    pub day: NaiveDate,
}

fn month_of(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let n = name.trim().to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|m| n.starts_with(m))
        .map(|i| i as u32 + 1)
}

/// Text between the end of the tag opened at `at` and the next `<`.
fn text_after(html: &str, at: usize) -> &str {
    let rest = &html[at..];
    let Some(open) = rest.find('>') else {
        return "";
    };
    let rest = &rest[open + 1..];
    // The month is wrapped in a <strong>.
    let rest = rest.strip_prefix("<strong>").unwrap_or(rest);
    rest.split('<').next().unwrap_or("").trim()
}

/// The Federal Reserve's FOMC calendar page: a panel per year
/// (`2026 FOMC Meetings`), a month cell then a date cell per meeting
/// (`27-28`, `17-18*` with projections, `Apr/May` + `30-1` across months).
/// The decision is the last day of the meeting.
pub fn fomc_calendar(html: &str) -> Option<Vec<Scheduled>> {
    const MONTH: &str = "fomc-meeting__month";
    const DATE: &str = "fomc-meeting__date";
    if !html.contains(MONTH) {
        return None;
    }
    let years: Vec<(usize, i32)> = html
        .match_indices(" FOMC Meetings")
        .filter_map(|(i, _)| {
            let y = html.get(i.checked_sub(4)?..i)?.parse::<i32>().ok()?;
            Some((i, y))
        })
        .collect();
    let mut out = Vec::new();
    let mut month: Option<u32> = None;
    let mut cells: Vec<(usize, bool)> = html
        .match_indices(MONTH)
        .map(|(i, _)| (i, true))
        .chain(html.match_indices(DATE).map(|(i, _)| (i, false)))
        .collect();
    cells.sort();
    for (at, is_month) in cells {
        let text = text_after(html, at);
        if is_month {
            // `Apr/May`: the meeting ends in the last month named.
            month = text.rsplit('/').next().and_then(month_of);
            continue;
        }
        let Some(m) = month.take() else { continue };
        let Some(&(_, year)) = years.iter().rev().find(|(i, _)| *i < at) else {
            continue;
        };
        // `17-18*`, `29-30 (unscheduled)`: digits and dashes lead, a note
        // in words is not a scheduled meeting.
        let lead: String = text
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '*')
            .collect();
        if lead.len() != text.len() || lead.is_empty() {
            continue;
        }
        let projections = lead.ends_with('*');
        let Some(day) = lead
            .trim_end_matches('*')
            .rsplit('-')
            .next()
            .and_then(|d| d.parse::<u32>().ok())
        else {
            continue;
        };
        let Some(date) = NaiveDate::from_ymd_opt(year, m, day) else {
            continue;
        };
        let title = if projections {
            "FOMC decision, with economic projections"
        } else {
            "FOMC decision"
        };
        out.push(Scheduled {
            external_id: format!("fomc:{date}"),
            label: title.to_string(),
            day: date,
        });
    }
    Some(out)
}

/// The ECB's calendar of the Governing Council: a `dd/mm/yyyy` date then
/// its description. Only the monetary policy meetings are kept, on their
/// decision day (`Day 2`, or a one-day meeting).
pub fn ecb_calendar(html: &str) -> Option<Vec<Scheduled>> {
    if !html.contains("Governing Council") {
        return None;
    }
    let mut texts: Vec<String> = Vec::new();
    let mut inside = false;
    let mut cur = String::new();
    for c in html.chars() {
        match c {
            '<' => {
                inside = true;
                let t = cur.trim();
                if !t.is_empty() {
                    texts.push(t.to_string());
                }
                cur.clear();
            }
            '>' => inside = false,
            _ if !inside => cur.push(c),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for pair in texts.windows(2) {
        let Ok(date) = NaiveDate::parse_from_str(&pair[0], "%d/%m/%Y") else {
            continue;
        };
        let what = pair[1].replace("&amp;", "&");
        let lower = what.to_ascii_lowercase();
        if !lower.contains("monetary policy meeting")
            || lower.contains("non-monetary")
            || lower.contains("day 1")
        {
            continue;
        }
        out.push(Scheduled {
            external_id: format!("ecb:{date}"),
            label: what,
            day: date,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fomc_page_gives_the_decision_days() {
        let html = r#"<h4><a id="1">2026 FOMC Meetings</a></h4>
<div class="fomc-meeting__month col-xs-5"><strong>January</strong></div>
<div class="fomc-meeting__date col-xs-4">27-28</div>
<div class="fomc-meeting--shaded fomc-meeting__month col-xs-5"><strong>March</strong></div>
<div class="fomc-meeting__date col-xs-4">17-18*</div>
<div class="fomc-meeting__month"><strong>Apr/May</strong></div>
<div class="fomc-meeting__date">30-1</div>
<div class="fomc-meeting__month"><strong>October</strong></div>
<div class="fomc-meeting__date">14 (unscheduled)</div>
<h4><a id="2">2027 FOMC Meetings</a></h4>
<div class="fomc-meeting__month"><strong>January</strong></div>
<div class="fomc-meeting__date">26-27</div>"#;
        let r = fomc_calendar(html).unwrap();
        let days: Vec<String> = r.iter().map(|e| e.day.to_string()).collect();
        assert_eq!(
            days,
            ["2026-01-28", "2026-03-18", "2026-05-01", "2027-01-27"]
        );
        assert_eq!(r[1].label, "FOMC decision, with economic projections");
        assert_eq!(r[0].external_id, "fomc:2026-01-28");
        assert!(fomc_calendar("<html></html>").is_none());
    }

    #[test]
    fn the_ecb_page_gives_the_monetary_policy_decisions_only() {
        let html = "<p>Governing Council</p><dl>\
<dt>25/11/2026</dt><dd>Governing Council of the ECB: non-monetary policy meeting (in Frankfurt)</dd>\
<dt>16/12/2026</dt><dd>Governing Council of the ECB: monetary policy meeting in Frankfurt (Day 1)</dd>\
<dt>17/12/2026</dt><dd>Governing Council of the ECB: monetary policy meeting in Frankfurt (Day 2), followed by press conference</dd>\
<dt>18/12/2026</dt><dd>General Council meeting of the ECB in Frankfurt</dd></dl>";
        let r = ecb_calendar(html).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].day.to_string(), "2026-12-17");
        assert_eq!(r[0].external_id, "ecb:2026-12-17");
        assert!(ecb_calendar("<html></html>").is_none());
    }
}
