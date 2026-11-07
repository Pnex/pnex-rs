//! Site navigation rules (site-recipes.md D323–D326): a declarative YAML
//! recipe bound to one web site, made of closed actions, played step by
//! step by the server browser. Configuration, never code. Pure, wasm-safe:
//! the backend parses the YAML into JSON, this module validates it.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Steps per recipe.
pub const STEPS_MAX: usize = 60;
/// Length of any string of a step (text, selector, path).
pub const STRING_MAX: usize = 500;
/// Off-site hosts a recipe may fetch a PDF from.
pub const ALLOW_HOSTS_MAX: usize = 10;
/// Parameters of a recipe.
pub const PARAMS_MAX: usize = 10;
/// Longest `wait`, ms.
pub const WAIT_MAX_MS: u32 = 15_000;
/// Name length.
pub const NAME_MAX: usize = 120;
/// Cards one `cards` extract keeps (D790).
pub const CARDS_MAX: u32 = 50;
/// Fields of a card.
pub const CARD_FIELDS_MAX: usize = 10;
/// Links one `for_each` opens (D790): a reader's pace, never a crawl.
pub const FOR_EACH_MAX: u32 = 10;
/// Steps played on each link of a `for_each`.
pub const FOR_EACH_STEPS_MAX: usize = 10;
/// Keys a `press` step may send (D789): validating or moving in a list,
/// never a shortcut.
pub const KEYS: [&str; 5] = ["Enter", "Tab", "Escape", "ArrowDown", "ArrowUp"];
/// Attributes a card field may read besides its text.
pub const CARD_ATTRS: [&str; 6] = ["href", "src", "title", "alt", "aria-label", "datetime"];

/// Locates an element: exactly one of `text`, `text_contains`,
/// `selector`, `role` (with an optional accessible `name`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Target {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A missing target is skipped instead of failing the run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WaitSpec {
    /// Fixed pause, ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u32>,
    /// Wait until the page shows this text (up to `timeout_ms`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct PdfSpec {
    /// First link whose text contains this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_text_contains: Option<String>,
    /// First link whose address contains this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href_contains: Option<String>,
    /// Name of the output (default `pdf-<step>`).
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ExtractSpec {
    /// Text of every element matching this CSS selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// Page text between the first marker and the next second one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub between: Option<[String; 2]>,
    /// The whole page text.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all_text: bool,
    /// One card per element matching a selector, each with named fields
    /// (D790): a list of results, a field per line of a card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cards: Option<CardsSpec>,
    /// Name of the output (default `extract-<step>`).
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

/// A list of cards (D790): `selector` finds each card, every field reads a
/// sub-element of it (the card itself without a selector).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct CardsSpec {
    pub selector: String,
    pub fields: BTreeMap<String, CardField>,
    /// Cards kept, in page order ([`CARDS_MAX`] at most).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
}

/// One field of a card: the text of a sub-element, or one of its
/// attributes ([`CARD_ATTRS`]); a link is read absolute.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct CardField {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attr: Option<String>,
}

/// Types into a search field (D789): the field is found by `selector` or
/// `role`, else the page's first search field; anything but a search field
/// (a password, a login form, a plain form field) is refused at play time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct TypeSpec {
    /// What is typed; usually a parameter (`{{query}}`).
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

/// Opens each link of a `cards` extract and plays `steps` on it (D790), in
/// card order, [`FOR_EACH_MAX`] links at most. The extracts of the steps are
/// named `<as>.<n>` (n from 1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ForEachSpec {
    /// Name of an earlier `cards` extract.
    pub from: String,
    /// Its field holding the link.
    pub link: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
    /// A step list again: the schema refers to it instead of unrolling it.
    #[cfg_attr(feature = "openapi", schema(no_recursion))]
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct CountMin {
    pub selector: String,
    pub min: u32,
}

/// Exactly one check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct AssertSpec {
    /// The page text contains this (case and accents ignored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_contains: Option<String>,
    /// A previous extract (by name) is not empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_empty: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count_min: Option<CountMin>,
    /// The current address contains this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_contains: Option<String>,
}

/// One closed action. No login, no script, no form input but a search
/// field (D254, D258, D789).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// A path or an address of the site.
    Open(String),
    Click(Target),
    /// `bottom`, or a CSS selector to bring into view.
    Scroll(String),
    Wait(WaitSpec),
    FollowPdf(PdfSpec),
    Extract(ExtractSpec),
    Assert(AssertSpec),
    /// Types into a search field (D789).
    Type(TypeSpec),
    /// One key of [`KEYS`] on the focused field (D789).
    Press(String),
    /// Each link of a list of cards (D790).
    ForEach(ForEachSpec),
}

impl Step {
    pub fn action(&self) -> &'static str {
        match self {
            Step::Open(_) => "open",
            Step::Click(_) => "click",
            Step::Scroll(_) => "scroll",
            Step::Wait(_) => "wait",
            Step::FollowPdf(_) => "follow_pdf",
            Step::Extract(_) => "extract",
            Step::Assert(_) => "assert",
            Step::Type(_) => "type",
            Step::Press(_) => "press",
            Step::ForEach(_) => "for_each",
        }
    }

    /// May be missing without failing the run.
    pub fn optional(&self) -> bool {
        match self {
            Step::Click(t) => t.optional,
            Step::Wait(w) => w.optional,
            Step::FollowPdf(p) => p.optional,
            Step::Extract(e) => e.optional,
            Step::Type(t) => t.optional,
            Step::Open(_)
            | Step::Scroll(_)
            | Step::Assert(_)
            | Step::Press(_)
            | Step::ForEach(_) => false,
        }
    }

    /// Short human label of what the step aims at (step list, journal).
    pub fn label(&self) -> String {
        fn target(t: &Target) -> String {
            t.text
                .clone()
                .or_else(|| t.text_contains.clone())
                .or_else(|| t.selector.clone())
                .or_else(|| {
                    t.role
                        .as_ref()
                        .map(|r| format!("{r} {}", t.name.as_deref().unwrap_or_default()))
                })
                .unwrap_or_default()
        }
        match self {
            Step::Open(p) | Step::Scroll(p) => p.clone(),
            Step::Click(t) => target(t),
            Step::Wait(w) => w
                .text
                .clone()
                .unwrap_or_else(|| format!("{} ms", w.ms.unwrap_or_default())),
            Step::FollowPdf(p) => p
                .link_text_contains
                .clone()
                .or_else(|| p.href_contains.clone())
                .unwrap_or_else(|| ".pdf".into()),
            Step::Extract(e) => e
                .between
                .as_ref()
                .map(|[a, b]| format!("{a} … {b}"))
                .or_else(|| e.selector.clone())
                .or_else(|| e.cards.as_ref().map(|c| c.selector.clone()))
                .unwrap_or_else(|| "*".into()),
            Step::Type(t) => t.text.clone(),
            Step::Press(k) => k.clone(),
            Step::ForEach(f) => format!("{} · {}", f.from, f.link),
            Step::Assert(a) => a
                .page_contains
                .clone()
                .or_else(|| a.not_empty.clone())
                .or_else(|| {
                    a.count_min
                        .as_ref()
                        .map(|c| format!("{} ≥ {}", c.selector, c.min))
                })
                .or_else(|| a.url_contains.clone())
                .unwrap_or_default(),
        }
    }
}

/// A recipe as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub name: String,
    /// Host or address of the site; normalised by [`site_host`].
    pub site: String,
    /// Off-site hosts `follow_pdf` may fetch from.
    #[serde(default)]
    pub allow_hosts: Vec<String>,
    /// Names the run fills in, used as `{{name}}` in the steps.
    #[serde(default)]
    pub params: Vec<String>,
    /// Browser endpoint of the org to play on (its name); the org default,
    /// else the platform browser, when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<String>,
    /// How long the run files (screenshots, PDFs) are kept, D161 wire
    /// format (`none`, `days:N`, `keep`); [`CAPTURES_DEFAULT`] when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captures: Option<String>,
    pub steps: Vec<Step>,
}

/// Retention of the run files of a recipe that names none (D268).
pub const CAPTURES_DEFAULT: crate::media_ingest::AudioRetention =
    crate::media_ingest::AudioRetention::Days(30);

impl Recipe {
    /// Retention of the run files (screenshots, PDFs); the text extracts
    /// stay in the journal for good.
    pub fn capture_retention(&self) -> crate::media_ingest::AudioRetention {
        self.captures
            .as_deref()
            .and_then(crate::media_ingest::AudioRetention::from_wire)
            .unwrap_or(CAPTURES_DEFAULT)
    }
}

/// One problem of a recipe: where (`steps.3`, `site`…) and a machine
/// token (`required`, `invalid`, `one_of`, `unknown_param:x`, `max_items:60`…).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Violation {
    pub path: String,
    pub token: String,
}

fn v(path: impl Into<String>, token: impl Into<String>) -> Violation {
    Violation {
        path: path.into(),
        token: token.into(),
    }
}

/// Host of a site: scheme, port, `www.` and path dropped, lower case.
/// `None` for anything but a plain http(s) host.
pub fn site_host(raw: &str) -> Option<String> {
    let s = raw.trim();
    let full = if s.contains("://") {
        s.to_string()
    } else {
        format!("https://{s}")
    };
    let u = url::Url::parse(&full).ok()?;
    if !matches!(u.scheme(), "http" | "https") || !u.username().is_empty() || u.password().is_some()
    {
        return None;
    }
    let host = u.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    (!host.is_empty() && host.len() <= 255).then_some(host)
}

/// Address a run opens first: the scheme and host as written, `https`
/// when none was given.
pub fn site_base(raw: &str) -> Option<String> {
    let s = raw.trim();
    let full = if s.contains("://") {
        s.to_string()
    } else {
        format!("https://{s}")
    };
    let u = url::Url::parse(&full).ok()?;
    site_host(raw)?;
    Some(format!(
        "{}://{}",
        u.scheme(),
        u.host_str()?.to_ascii_lowercase()
    ))
}

/// `{{name}}` placeholders of a string.
fn placeholders(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("{{") {
        let Some(j) = rest[i + 2..].find("}}") else {
            break;
        };
        out.push(rest[i + 2..i + 2 + j].trim().to_string());
        rest = &rest[i + 2 + j + 2..];
    }
    out
}

/// Replaces `{{name}}` by its value (unknown names are left as is:
/// [`validate`] already refused them).
pub fn fill(s: &str, params: &BTreeMap<String, String>) -> String {
    let mut out = s.to_string();
    for (k, val) in params {
        out = out.replace(&format!("{{{{{k}}}}}"), val);
        out = out.replace(&format!("{{{{ {k} }}}}"), val);
    }
    out
}

fn strings(step: &Step) -> Vec<&str> {
    fn t(t: &Target) -> Vec<&str> {
        [&t.text, &t.text_contains, &t.selector, &t.role, &t.name]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect()
    }
    match step {
        Step::Open(s) | Step::Scroll(s) => vec![s],
        Step::Click(x) => t(x),
        Step::Wait(w) => w.text.iter().map(String::as_str).collect(),
        Step::FollowPdf(p) => [&p.link_text_contains, &p.href_contains, &p.name]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect(),
        Step::Extract(e) => {
            let mut out: Vec<&str> = [&e.selector, &e.name]
                .into_iter()
                .flatten()
                .map(String::as_str)
                .collect();
            if let Some([a, b]) = &e.between {
                out.extend([a.as_str(), b.as_str()]);
            }
            if let Some(c) = &e.cards {
                out.push(&c.selector);
                for (k, f) in &c.fields {
                    out.push(k);
                    out.extend(
                        [&f.selector, &f.attr]
                            .into_iter()
                            .flatten()
                            .map(String::as_str),
                    );
                }
            }
            out
        }
        Step::Type(t) => [Some(&t.text), t.selector.as_ref(), t.role.as_ref()]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect(),
        Step::Press(k) => vec![k],
        Step::ForEach(f) => vec![&f.from, &f.link],
        Step::Assert(a) => {
            let mut out: Vec<&str> = [&a.page_contains, &a.not_empty, &a.url_contains]
                .into_iter()
                .flatten()
                .map(String::as_str)
                .collect();
            if let Some(c) = &a.count_min {
                out.push(&c.selector);
            }
            out
        }
    }
}

/// Marks, in the names seen so far, an extract that is a list of cards.
const CARDS_MARK: &str = "cards:";

fn cards_of(names: &[String], name: &str) -> bool {
    names
        .iter()
        .any(|n| n.strip_prefix(CARDS_MARK) == Some(name))
}

fn field_name_ok(k: &str) -> bool {
    !k.is_empty() && k.len() <= 40 && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Number of `Some`/`true` among the alternatives of a step.
fn chosen(flags: &[bool]) -> usize {
    flags.iter().filter(|f| **f).count()
}

fn check_step(i: usize, step: &Step, params: &[String], names: &mut Vec<String>) -> Vec<Violation> {
    let at = format!("steps.{i}");
    let mut out = Vec::new();
    for s in strings(step) {
        if s.trim().is_empty() {
            out.push(v(&at, "required"));
        }
        if s.chars().count() > STRING_MAX {
            out.push(v(&at, format!("max_length:{STRING_MAX}")));
        }
        for p in placeholders(s) {
            if !params.contains(&p) {
                out.push(v(&at, format!("unknown_param:{p}")));
            }
        }
    }
    let one_of = match step {
        Step::Click(t) => chosen(&[
            t.text.is_some(),
            t.text_contains.is_some(),
            t.selector.is_some(),
            t.role.is_some(),
        ]),
        Step::Wait(w) => chosen(&[w.ms.is_some(), w.text.is_some()]),
        Step::FollowPdf(p) => chosen(&[p.link_text_contains.is_some(), p.href_contains.is_some()]),
        Step::Extract(e) => chosen(&[
            e.selector.is_some(),
            e.between.is_some(),
            e.all_text,
            e.cards.is_some(),
        ]),
        Step::Type(t) => usize::from(!(t.selector.is_some() && t.role.is_some())),
        Step::Assert(a) => chosen(&[
            a.page_contains.is_some(),
            a.not_empty.is_some(),
            a.count_min.is_some(),
            a.url_contains.is_some(),
        ]),
        Step::Open(_) | Step::Scroll(_) | Step::Press(_) | Step::ForEach(_) => 1,
    };
    if one_of != 1 {
        out.push(v(&at, "one_of"));
    }
    match step {
        Step::Wait(w) if w.ms.or(w.timeout_ms).is_some_and(|ms| ms > WAIT_MAX_MS) => {
            out.push(v(&at, format!("max:{WAIT_MAX_MS}")));
        }
        Step::Press(k) if !KEYS.contains(&k.as_str()) => out.push(v(&at, "invalid")),
        Step::Extract(ExtractSpec { cards: Some(c), .. })
            if c.fields.is_empty()
                || c.fields.len() > CARD_FIELDS_MAX
                || c.max.is_some_and(|m| m == 0 || m > CARDS_MAX)
                || c.fields.iter().any(|(k, f)| {
                    !field_name_ok(k) || f.attr.as_deref().is_some_and(|a| !CARD_ATTRS.contains(&a))
                }) =>
        {
            out.push(v(&at, "invalid"));
        }
        Step::ForEach(f) => {
            if !cards_of(names, &f.from) {
                out.push(v(&at, format!("unknown_extract:{}", f.from)));
            }
            if f.max.is_some_and(|m| m == 0 || m > FOR_EACH_MAX) {
                out.push(v(&at, format!("max:{FOR_EACH_MAX}")));
            }
            if f.steps.is_empty() {
                out.push(v(&at, "required"));
            } else if f.steps.len() > FOR_EACH_STEPS_MAX {
                out.push(v(&at, format!("max_items:{FOR_EACH_STEPS_MAX}")));
            }
            // One level, and each link is the page: no nested loop, no open.
            let mut inner = names.clone();
            for (j, sub) in f.steps.iter().enumerate() {
                if matches!(sub, Step::ForEach(_) | Step::Open(_)) {
                    out.push(v(format!("{at}.steps.{j}"), "invalid"));
                    continue;
                }
                for mut x in check_step(j, sub, params, &mut inner) {
                    x.path = x.path.replacen("steps.", &format!("{at}.steps."), 1);
                    out.push(x);
                }
            }
        }
        Step::Extract(_) | Step::FollowPdf(_) => {
            if let Step::Extract(ExtractSpec { cards: Some(_), .. }) = step {
                names.push(format!(
                    "{CARDS_MARK}{}",
                    step.output_name(i).unwrap_or_default()
                ));
            }
            names.extend(step.output_name(i));
        }
        Step::Assert(AssertSpec {
            not_empty: Some(n), ..
        }) if !names.contains(n) => out.push(v(&at, format!("unknown_extract:{n}"))),
        _ => {}
    }
    out
}

/// Parses and checks a recipe from its YAML-compatible JSON. Each step is
/// read on its own so a bad step points at its index.
pub fn validate(doc: &Value) -> Result<Recipe, Vec<Violation>> {
    let mut out = Vec::new();
    let Some(obj) = doc.as_object() else {
        return Err(vec![v("", "invalid")]);
    };
    for k in obj.keys() {
        if !matches!(
            k.as_str(),
            "name" | "site" | "allow_hosts" | "params" | "browser" | "captures" | "steps"
        ) {
            out.push(v(k, "unknown"));
        }
    }
    let text = |k: &str| {
        obj.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
    };
    let name = text("name");
    if name.is_empty() {
        out.push(v("name", "required"));
    } else if name.chars().count() > NAME_MAX {
        out.push(v("name", format!("max_length:{NAME_MAX}")));
    }
    if text("site").is_empty() {
        out.push(v("site", "required"));
    } else if site_host(text("site")).is_none() {
        out.push(v("site", "invalid"));
    }
    let list = |k: &str| -> Result<Vec<String>, ()> {
        match obj.get(k) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(x) => serde_json::from_value(x.clone()).map_err(|_| ()),
        }
    };
    let allow_hosts = list("allow_hosts").unwrap_or_else(|_| {
        out.push(v("allow_hosts", "invalid"));
        Vec::new()
    });
    if allow_hosts.len() > ALLOW_HOSTS_MAX {
        out.push(v("allow_hosts", format!("max_items:{ALLOW_HOSTS_MAX}")));
    }
    for (i, h) in allow_hosts.iter().enumerate() {
        if site_host(h).is_none() {
            out.push(v(format!("allow_hosts.{i}"), "invalid"));
        }
    }
    let params = list("params").unwrap_or_else(|_| {
        out.push(v("params", "invalid"));
        Vec::new()
    });
    if params.len() > PARAMS_MAX {
        out.push(v("params", format!("max_items:{PARAMS_MAX}")));
    }
    for (i, p) in params.iter().enumerate() {
        let ok = !p.is_empty()
            && p.len() <= 40
            && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ok {
            out.push(v(format!("params.{i}"), "invalid"));
        }
    }
    let browser = match obj.get("browser") {
        None | Some(Value::Null) => None,
        Some(Value::String(b)) if !b.trim().is_empty() && b.chars().count() <= 255 => {
            Some(b.trim().to_string())
        }
        Some(_) => {
            out.push(v("browser", "invalid"));
            None
        }
    };
    let captures = match obj.get("captures") {
        None | Some(Value::Null) => None,
        Some(Value::String(c))
            if crate::media_ingest::AudioRetention::from_wire(c.trim()).is_some() =>
        {
            Some(c.trim().to_string())
        }
        Some(_) => {
            out.push(v("captures", "invalid"));
            None
        }
    };
    let mut steps = Vec::new();
    match obj.get("steps").and_then(Value::as_array) {
        None => out.push(v("steps", "required")),
        Some(raw) if raw.is_empty() => out.push(v("steps", "required")),
        Some(raw) if raw.len() > STEPS_MAX => {
            out.push(v("steps", format!("max_items:{STEPS_MAX}")))
        }
        Some(raw) => {
            let mut names = Vec::new();
            for (i, s) in raw.iter().enumerate() {
                match serde_json::from_value::<Step>(s.clone()) {
                    Ok(step) => {
                        out.extend(check_step(i, &step, &params, &mut names));
                        steps.push(step);
                    }
                    Err(_) => out.push(v(format!("steps.{i}"), "invalid")),
                }
            }
        }
    }
    if !out.is_empty() {
        return Err(out);
    }
    Ok(Recipe {
        name: name.to_string(),
        site: text("site").to_string(),
        allow_hosts,
        params,
        browser,
        captures,
        steps,
    })
}

// ───────────────────────────── page text ─────────────────────────────

/// Folded text without any whitespace (U+00A0, U+202F, U+2009 included),
/// each folded char paired with the index of the original char it comes
/// from: "RS 7511" and "RS7511" compare equal, and a ligature folded to two
/// chars keeps both of them.
fn squash(text: &str) -> Vec<(char, usize)> {
    text.chars()
        .enumerate()
        .filter(|(_, c)| !c.is_whitespace())
        .flat_map(|(i, c)| {
            crate::fold::fold(&c.to_string())
                .chars()
                .map(move |f| (f, i))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Folded char stream index where `needle` starts, from stream index `from`.
fn find_squashed(hay: &[(char, usize)], needle: &str, from: usize) -> Option<usize> {
    let n: Vec<char> = squash(needle).into_iter().map(|(c, _)| c).collect();
    if n.is_empty() || hay.len() < n.len() || from > hay.len() - n.len() {
        return None;
    }
    (from..=hay.len() - n.len()).find(|&i| {
        hay[i..i + n.len()]
            .iter()
            .map(|(c, _)| *c)
            .eq(n.iter().copied())
    })
}

/// `needle` in `text`, ignoring case, accents and whitespace.
pub fn contains_loose(text: &str, needle: &str) -> bool {
    find_squashed(&squash(text), needle, 0).is_some()
}

/// Text between the first `a` and the next `b` after it (case, accents and
/// whitespace ignored); `None` when `a` is missing. Without `b`, up to the end.
pub fn between(text: &str, a: &str, b: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let hay = squash(text);
    let a_len = squash(a).len();
    let k = find_squashed(&hay, a, 0)?;
    // Original index right after the last char of the matched marker.
    let start = hay[k + a_len - 1].1 + 1;
    let end = find_squashed(&hay, b, k + a_len)
        .map(|j| hay[j].1)
        .unwrap_or(chars.len());
    Some(
        chars[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_string(),
    )
}

// ───────────────────────────── page side ─────────────────────────────

/// The code a rule runs inside the visited page, shared by both players
/// (D780): the server browser evaluates it over CDP, the browser extension
/// imports the same file (`js/rule_page.js`).
pub const PAGE_JS: &str = include_str!("../js/rule_page.js");

/// An expression calling `function` of [`PAGE_JS`] with JSON arguments, for
/// an engine that evaluates source (CDP).
pub fn page_call(function: &str, args: &[Value]) -> String {
    let src = PAGE_JS.replace("export function", "function");
    let args: Vec<String> = args.iter().map(Value::to_string).collect();
    format!(
        "(() => {{\n{src}\nreturn {function}({});\n}})()",
        args.join(", ")
    )
}

// ───────────────────────────── browser endpoints ─────────────────────────────

/// A CDP browser of the org (site-recipes.md D325). The token is a vault
/// reference, never its value (R4, R16).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BrowserEndpoint {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    /// `wss://…`, `ws://…`, or the `http(s)://host:port` of `/json/version`.
    pub cdp_url: String,
    pub token: Option<crate::SecretFieldView>,
    pub max_connections: i32,
    pub is_default: bool,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Create or replace a browser endpoint; `token: None` keeps the current one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BrowserEndpointInput {
    pub name: String,
    pub cdp_url: String,
    #[serde(default)]
    pub token: Option<crate::SecretFieldInput>,
    #[serde(default = "default_connections")]
    pub max_connections: i32,
    #[serde(default)]
    pub is_default: bool,
}

fn default_connections() -> i32 {
    2
}

/// One check of an endpoint test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BrowserCheck {
    /// `version`, `context`, `navigate`, `internal-refused`.
    pub check: String,
    pub ok: bool,
    /// Diagnostic as is, not translated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub ms: u64,
}

/// Answer of an endpoint test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BrowserTestResult {
    pub ok: bool,
    /// `Browser` of `/json/version`.
    pub engine: Option<String>,
    pub protocol: Option<String>,
    pub latency_ms: u64,
    pub checks: Vec<BrowserCheck>,
}

// ───────────────────────────── visual editing ─────────────────────────────

/// The closed actions, in palette order.
pub const ACTIONS: [&str; 9] = [
    "open",
    "click",
    "type",
    "press",
    "scroll",
    "wait",
    "follow_pdf",
    "extract",
    "assert",
];

/// A new step of an action, with neutral values to fill in.
pub fn new_step(action: &str) -> Option<Step> {
    Some(match action {
        "open" => Step::Open("/".into()),
        "click" => Step::Click(Target {
            text: Some(String::new()),
            ..Target::default()
        }),
        "scroll" => Step::Scroll("bottom".into()),
        "wait" => Step::Wait(WaitSpec {
            ms: Some(1000),
            ..WaitSpec::default()
        }),
        "follow_pdf" => Step::FollowPdf(PdfSpec {
            link_text_contains: Some(String::new()),
            ..PdfSpec::default()
        }),
        "extract" => Step::Extract(ExtractSpec {
            all_text: true,
            ..ExtractSpec::default()
        }),
        "assert" => Step::Assert(AssertSpec {
            page_contains: Some(String::new()),
            ..AssertSpec::default()
        }),
        "type" => Step::Type(TypeSpec {
            text: String::new(),
            ..TypeSpec::default()
        }),
        "press" => Step::Press("Enter".into()),
        _ => return None,
    })
}

/// YAML flow value of JSON (keys bare, scalars as JSON: valid YAML).
fn flow(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            // What the step aims at first, its name and `optional` last.
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort_by_key(|k| matches!(k.as_str(), "as" | "optional"));
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|k| format!("{k}: {}", flow(&m[*k])))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Value::Array(a) => format!("[{}]", a.iter().map(flow).collect::<Vec<_>>().join(", ")),
        other => other.to_string(),
    }
}

/// The recipe as YAML, one step per line (what the visual editor saves).
/// Comments of a hand-written YAML are not kept.
pub fn to_yaml(r: &Recipe) -> String {
    let s = |x: &str| Value::String(x.to_string()).to_string();
    let mut out = format!("name: {}\nsite: {}\n", s(&r.name), s(&r.site));
    if !r.params.is_empty() {
        out += &format!("params: {}\n", flow(&serde_json::json!(r.params)));
    }
    if !r.allow_hosts.is_empty() {
        out += &format!("allow_hosts: {}\n", flow(&serde_json::json!(r.allow_hosts)));
    }
    if let Some(b) = &r.browser {
        out += &format!("browser: {}\n", s(b));
    }
    if let Some(c) = &r.captures {
        out += &format!("captures: {}\n", s(c));
    }
    out += "steps:\n";
    for step in &r.steps {
        if let Ok(Value::Object(m)) = serde_json::to_value(step) {
            for (action, v) in m {
                out += &format!("  - {action}: {}\n", flow(&v));
            }
        }
    }
    out
}

// ───────────────────────────── API ─────────────────────────────

/// A saved recipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SiteRecipe {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    /// Normalised host of the site.
    pub site: String,
    pub name: String,
    pub yaml: String,
    pub version: i32,
    /// The parsed recipe (step list of the studio).
    pub recipe: Option<Recipe>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// One saved version of a recipe (D221).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SiteRecipeVersion {
    pub version: i32,
    pub yaml: String,
    /// E-mail of the author, when known.
    pub author: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
}

/// Create or replace a recipe.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SiteRecipeInput {
    pub yaml: String,
    /// Version the edit was made on (update only): another save in
    /// between answers 409.
    #[serde(default)]
    pub expected_version: Option<i32>,
}

/// Check a draft without saving it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SiteRecipeDraft {
    pub yaml: String,
}

/// A web site of the org: the recipes anchored on it and its last run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WebSite {
    pub site: String,
    pub recipes: i64,
    /// Pages of the site found by a `crawl_site` mission (D307).
    #[serde(default)]
    pub pages: i64,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub last_run_at: Option<String>,
    pub last_status: Option<RunStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Ok,
    Failed,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Ok => "ok",
            RunStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "ok" => RunStatus::Ok,
            "running" => RunStatus::Running,
            _ => RunStatus::Failed,
        }
    }
}

/// Why a step failed (machine token, `run-reason-<token>` in the UI).
/// Name of what step `i` brings back (`extract`, `follow_pdf`), if any.
impl Step {
    pub fn output_name(&self, i: usize) -> Option<String> {
        match self {
            Step::Extract(e) => Some(e.name.clone().unwrap_or_else(|| format!("extract-{i}"))),
            Step::FollowPdf(p) => Some(p.name.clone().unwrap_or_else(|| format!("pdf-{i}"))),
            _ => None,
        }
    }
}

pub mod reason {
    pub const TARGET_NOT_FOUND: &str = "target-not-found";
    pub const ASSERT_FAILED: &str = "assert-failed";
    pub const HTTP_STATUS: &str = "http-status";
    pub const RATE_LIMITED: &str = "rate-limited";
    pub const TIMEOUT: &str = "timeout";
    pub const ROBOTS: &str = "robots-disallowed";
    pub const OFF_SITE: &str = "off-site";
    pub const EGRESS: &str = "egress-blocked";
    pub const NOT_A_PDF: &str = "not-a-pdf";
    pub const TOO_LARGE: &str = "too-large";
    pub const BROWSER: &str = "browser-unavailable";
    pub const PARAM_MISSING: &str = "param-missing";
    pub const INTERRUPTED: &str = "interrupted";
    /// A `type` step aimed at a field that is not a search (D789).
    pub const NOT_A_SEARCH_FIELD: &str = "not-a-search-field";
    /// Step taken over from the run a resumed run started from.
    pub const REUSED: &str = "reused";
    pub const ALL: &[&str] = &[
        TARGET_NOT_FOUND,
        ASSERT_FAILED,
        HTTP_STATUS,
        RATE_LIMITED,
        TIMEOUT,
        ROBOTS,
        OFF_SITE,
        EGRESS,
        NOT_A_PDF,
        TOO_LARGE,
        BROWSER,
        PARAM_MISSING,
        INTERRUPTED,
        NOT_A_SEARCH_FIELD,
        REUSED,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Ok,
    Skipped,
    Failed,
}

/// One played step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunStep {
    pub index: u32,
    pub action: String,
    pub label: String,
    pub status: StepStatus,
    /// Token of [`reason`] when failed or skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Diagnostic as is (HTTP status, address refused…), not translated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Page address after the step.
    pub url: String,
    pub ms: u64,
    /// File of the step's screenshot (`GET …/runs/{id}/files/{file}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExtractKind {
    Text,
    Pdf,
    /// A list of cards (D790): `cards` holds them, `text` their JSON.
    Cards,
}

/// What a run brought back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunExtract {
    pub name: String,
    pub kind: ExtractKind,
    /// Text read (the PDF's text for a PDF), capped.
    pub text: String,
    pub chars: u64,
    pub sha256: String,
    /// Address the PDF was fetched from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// File of the PDF (`GET …/runs/{id}/files/{file}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// The cards of a `cards` extract, field by field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<BTreeMap<String, String>>,
}

/// One run of a recipe (journal row; the single read adds the diff).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SiteRecipeRun {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub recipe_id: Option<String>,
    /// Saved version played; `null` for a draft.
    pub recipe_version: Option<i32>,
    pub site: String,
    pub name: String,
    /// `studio` or `flow`.
    pub trigger: String,
    pub status: RunStatus,
    pub failed_step: Option<u32>,
    pub reason: Option<String>,
    pub steps: Vec<RunStep>,
    pub extracts: Vec<RunExtract>,
    /// Per extract name: changes since the previous finished run of the
    /// same recipe (single read only).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub changes: BTreeMap<String, crate::text_diff::TextDiff>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub started_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub finished_at: Option<String>,
    /// Where it plays (D781).
    #[serde(default)]
    pub engine: RunEngine,
}

/// Engine of a rule run (D781): the server browser, or a browser agent of
/// the org playing it in the user's own session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunEngine {
    #[default]
    ServerBrowser,
    Extension,
}

impl RunEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ServerBrowser => "server_browser",
            Self::Extension => "extension",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "server_browser" => Some(Self::ServerBrowser),
            "extension" => Some(Self::Extension),
            _ => None,
        }
    }
}

/// What a browser agent plays (D783): sealed by the server when it leases
/// the run, played to the end without asking anything back. Its pages are
/// checked again on return against `site` and `allow_hosts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulePlan {
    pub run_id: String,
    /// Address the run opens first ([`site_base`]).
    pub base: String,
    /// Host of the site ([`site_host`]).
    pub site: String,
    pub recipe: Recipe,
    /// Values of the rule's parameters, already checked.
    pub params: BTreeMap<String, String>,
}

/// Play a recipe: a saved one (`recipe_id`) or a draft (`yaml`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunInput {
    #[serde(default)]
    pub recipe_id: Option<String>,
    /// Draft to play instead of the saved version.
    #[serde(default)]
    pub yaml: Option<String>,
    /// Values of the recipe's `params`.
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    /// Start at a step instead of the first one (studio only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<RunResume>,
    /// Play on a browser agent of the org instead of the server browser
    /// (D781); the caller's own browser, else a common one (D550).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<RunEngine>,
}

/// Resume point of a run: starts on the page an earlier run of the same
/// site was on after step `step - 1`, and plays from `step` on. The earlier
/// steps and their extracts are taken over from that run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunResume {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub run_id: String,
    /// First step played (>= 1).
    pub step: u32,
}

// ─────────────────────────── flow node (D327) ───────────────────────────

/// Output ports of the `site_recipe` node: the run passed, failed again,
/// or failed right after a passing run (wire that one to a notification).
pub const NODE_PORT_OK: usize = 0;
pub const NODE_PORT_FAILED: usize = 1;
pub const NODE_PORT_BROKE: usize = 2;
pub const NODE_PORT_COUNT: usize = 3;

/// Config of a `site_recipe` flow node: one saved recipe of the org,
/// played on the server browser at each message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteRecipeNodeConfig {
    #[serde(default)]
    pub recipe_id: String,
    /// Values of the recipe's `params`; `msg.params` overrides them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

impl SiteRecipeNodeConfig {
    /// Shape errors, shown while editing (an unset recipe is allowed).
    pub fn check(&self) -> Option<(&'static str, &'static str)> {
        let bad = |m| Some(("site_recipe_config_invalid", m));
        if !self.recipe_id.is_empty() && uuid::Uuid::parse_str(&self.recipe_id).is_err() {
            return bad("the recipe id is not a uuid");
        }
        if self.params.len() > PARAMS_MAX
            || self
                .params
                .iter()
                .any(|(k, v)| k.len() > STRING_MAX || v.chars().count() > STRING_MAX)
        {
            return bad("at most 10 parameters of 500 characters");
        }
        None
    }

    /// Deploy gate: a recipe must be chosen.
    pub fn check_deployable(&self) -> Option<(&'static str, &'static str)> {
        self.check().or_else(|| {
            self.recipe_id
                .is_empty()
                .then_some(("site_recipe_config_unset", "choose a navigation rule"))
        })
    }
}

/// When a recipe plays next: the deployed flows whose triggers reach one
/// of its nodes (D326, no scheduler of its own).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SiteRecipeSchedule {
    pub flows: Vec<ScheduledFlow>,
    /// Next cron times across those flows, soonest first (UTC).
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<String>, format = DateTime))]
    pub next: Vec<String>,
}

/// A deployed flow that plays the recipe, and its triggers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScheduledFlow {
    pub flow_id: i64,
    pub name: String,
    /// Cron expressions of its inject nodes (UTC).
    pub cron: Vec<String>,
    /// Repeat intervals of its inject nodes, seconds.
    pub every_secs: Vec<f64>,
}

/// `POST /internal/flow/site-recipe`: the deployed node and the message's
/// parameter values (the recipe is read from the deployed config).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteRecipeNodeRequest {
    pub node: crate::ontology::flow::FlowNodeRef,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

/// Answer: output port, the run's manifest, the extracts' text by name,
/// and a refusal code when the run could not start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteRecipeNodeReply {
    pub port: usize,
    #[serde(default)]
    pub manifest: Value,
    #[serde(default)]
    pub payload: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub args: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn site_fixture_training_org() -> Value {
        json!({
            "name": "programme-rs0101",
            "site": "https://www.la-strat.fr",
            "params": ["code_certif"],
            "steps": [
                {"open": "/formations/"},
                {"click": {"text": "Accepter", "optional": true}},
                {"click": {"text_contains": "{{code_certif}}"}},
                {"follow_pdf": {"link_text_contains": "programme", "optional": true}},
                {"extract": {"between": ["Programme", "Modalités"], "as": "programme"}},
                {"assert": {"not_empty": "programme"}}
            ]
        })
    }

    #[test]
    fn a_valid_recipe_parses() {
        let r = validate(&site_fixture_training_org()).unwrap();
        assert_eq!(r.steps.len(), 6);
        assert!(r.steps[1].optional());
        assert_eq!(r.steps[4].label(), "Programme … Modalités");
    }

    #[test]
    fn captures_retention_defaults_and_round_trips() {
        let r = validate(&site_fixture_training_org()).unwrap();
        assert_eq!(r.capture_retention(), CAPTURES_DEFAULT);
        let mut d = site_fixture_training_org();
        d["captures"] = json!("days:0");
        assert!(validate(&d)
            .unwrap_err()
            .contains(&v("captures", "invalid")));
        d["captures"] = json!("keep");
        let r = validate(&d).unwrap();
        assert_eq!(
            r.capture_retention(),
            crate::media_ingest::AudioRetention::Keep
        );
        assert!(to_yaml(&r).contains("captures: \"keep\"\n"));
    }

    #[test]
    fn bad_steps_point_at_their_index() {
        let mut d = site_fixture_training_org();
        d["steps"][2] = json!({"click": {"text": "a", "selector": "b"}});
        d["steps"][3] = json!({"type_text": "x"});
        d["steps"][5] = json!({"assert": {"not_empty": "nope"}});
        let errs = validate(&d).unwrap_err();
        assert!(errs.contains(&v("steps.2", "one_of")));
        assert!(errs.contains(&v("steps.3", "invalid")));
        assert!(errs.contains(&v("steps.5", "unknown_extract:nope")));
    }

    #[test]
    fn unknown_params_are_refused_and_known_ones_filled() {
        let mut d = site_fixture_training_org();
        d["params"] = json!([]);
        assert!(validate(&d)
            .unwrap_err()
            .contains(&v("steps.2", "unknown_param:code_certif")));
        let p = BTreeMap::from([("code_certif".to_string(), "RS0101".to_string())]);
        assert_eq!(
            fill("x {{code_certif}} {{ code_certif }}", &p),
            "x RS0101 RS0101"
        );
    }

    #[test]
    fn every_new_step_is_an_editable_action() {
        for a in ACTIONS {
            assert_eq!(new_step(a).map(|s| s.action()), Some(a));
        }
        let y = to_yaml(&validate(&site_fixture_training_org()).unwrap());
        assert!(
            y.contains("  - click: {text: \"Accepter\", optional: true}\n"),
            "{y}"
        );
        assert!(
            y.contains(
                "  - extract: {between: [\"Programme\", \"Modalités\"], as: \"programme\"}\n"
            ),
            "{y}"
        );
    }

    /// A people search of a network (D788–D790): typed in its search bar,
    /// the result cards read, each profile opened.
    fn people_search() -> Value {
        json!({
            "name": "people-search",
            "site": "https://www.example.org",
            "params": ["query"],
            "steps": [
                {"open": "/"},
                {"type": {"text": "{{query}}", "role": "combobox"}},
                {"press": "Enter"},
                {"wait": {"text": "results"}},
                {"extract": {"cards": {"selector": "li.result", "max": 5, "fields": {
                    "name": {"selector": ".name"},
                    "headline": {"selector": ".headline"},
                    "link": {"selector": "a.profile", "attr": "href"}
                }}, "as": "results"}},
                {"for_each": {"from": "results", "link": "link", "max": 3, "steps": [
                    {"extract": {"selector": "h1", "as": "name"}},
                    {"extract": {"all_text": true, "as": "profile"}}
                ]}}
            ]
        })
    }

    #[test]
    fn a_search_typed_then_its_cards_followed_is_a_valid_rule() {
        let r = validate(&people_search()).unwrap();
        assert_eq!(r.steps[1].action(), "type");
        assert_eq!(r.steps[1].label(), "{{query}}");
        assert_eq!(r.steps[5].label(), "results · link");
        let y = to_yaml(&r);
        assert!(y.contains("  - press: \"Enter\"\n"), "{y}");
        assert!(y.contains("for_each"), "{y}");
    }

    #[test]
    fn typing_keys_cards_and_loops_stay_bounded() {
        let bad = |path: &str, step: Value, want: &str| {
            let mut d = people_search();
            let i: usize = path.parse().unwrap();
            d["steps"][i] = step;
            let errs = validate(&d).unwrap_err();
            assert!(
                errs.iter().any(|e| e.token == want),
                "{path} {want}: {errs:?}"
            );
        };
        bad("2", json!({"press": "Control+a"}), "invalid");
        bad(
            "1",
            json!({"type": {"text": "x", "role": "a", "selector": "b"}}),
            "one_of",
        );
        bad(
            "1",
            json!({"type": {"text": "{{password}}"}}),
            "unknown_param:password",
        );
        bad(
            "4",
            json!({"extract": {"cards": {"selector": "li", "fields": {"x": {"attr": "onclick"}}}, "as": "results"}}),
            "invalid",
        );
        bad(
            "4",
            json!({"extract": {"cards": {"selector": "li", "fields": {}}, "as": "results"}}),
            "invalid",
        );
        bad(
            "5",
            json!({"for_each": {"from": "nope", "link": "link", "steps": [{"extract": {"all_text": true}}]}}),
            "unknown_extract:nope",
        );
        bad(
            "5",
            json!({"for_each": {"from": "results", "link": "link", "max": 99, "steps": [{"extract": {"all_text": true}}]}}),
            "max:10",
        );
        // No loop in a loop, no open: each link is the page.
        let mut d = people_search();
        d["steps"][5]["for_each"]["steps"][0] = json!({"open": "/x"});
        let errs = validate(&d).unwrap_err();
        assert!(errs.contains(&v("steps.5.steps.0", "invalid")), "{errs:?}");
        // A text extract is not a list of cards.
        let mut d = people_search();
        d["steps"][4] = json!({"extract": {"all_text": true, "as": "results"}});
        assert!(validate(&d).is_err());
    }

    #[test]
    fn between_ignores_case_and_accents() {
        let t = "Accueil\nPROGRAMME de la formation\nModule 1 (4 h)\nModalités : présentiel";
        assert_eq!(
            between(t, "programme", "modalites").as_deref(),
            Some("de la formation\nModule 1 (4 h)")
        );
        assert_eq!(between(t, "absent", "x"), None);
        assert_eq!(
            between(t, "Module 1", "nothing").as_deref(),
            Some("(4 h)\nModalités : présentiel")
        );
    }

    #[test]
    fn matching_ignores_no_break_spaces_and_ligatures() {
        // U+00A0 in the page and in the marker, U+202F before the colon.
        let t = "Accueil\nContenu\u{a0}de la formation\nPartie 1\nCertification RS\u{a0}7511\u{202f}: oui";
        assert_eq!(
            between(t, "Contenu de\u{a0}la formation", "Certification").as_deref(),
            Some("Partie 1")
        );
        assert!(contains_loose(t, "RS7511"));
        assert!(contains_loose(t, "rs 7511 : OUI"));
        assert!(!contains_loose(t, "RS7512"));
        // Ligatures fold to two chars on both sides; the cut lands after the marker.
        let l = "Le cœur du métier\nSuite";
        assert_eq!(between(l, "COEUR", "suite").as_deref(), Some("du métier"));
        assert_eq!(between(l, "cœur du", "").as_deref(), Some("métier\nSuite"));
    }

    #[test]
    fn sites_normalise_to_one_host() {
        for raw in [
            "la-strat.fr",
            "https://www.la-strat.fr/x?y",
            "http://LA-STRAT.fr:8080",
        ] {
            assert_eq!(site_host(raw).as_deref(), Some("la-strat.fr"));
        }
        assert_eq!(site_host("ftp://x.fr"), None);
        assert_eq!(
            site_base("www.la-strat.fr").as_deref(),
            Some("https://www.la-strat.fr")
        );
    }

    #[test]
    fn the_flow_node_needs_a_recipe_to_deploy() {
        let mut c = SiteRecipeNodeConfig::default();
        assert!(c.check().is_none());
        assert_eq!(c.check_deployable().unwrap().0, "site_recipe_config_unset");
        c.recipe_id = "nope".into();
        assert_eq!(c.check().unwrap().0, "site_recipe_config_invalid");
        c.recipe_id = "6f1c2b7e-1d2a-4c3b-9e8f-0a1b2c3d4e5f".into();
        assert!(c.check_deployable().is_none());
    }
}
