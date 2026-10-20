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
    /// Name of the output (default `extract-<step>`).
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
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

/// One closed action. No form input, no login, no script (D254, D258).
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
        }
    }

    /// May be missing without failing the run.
    pub fn optional(&self) -> bool {
        match self {
            Step::Click(t) => t.optional,
            Step::Wait(w) => w.optional,
            Step::FollowPdf(p) => p.optional,
            Step::Extract(e) => e.optional,
            Step::Open(_) | Step::Scroll(_) | Step::Assert(_) => false,
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
                .unwrap_or_else(|| "*".into()),
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
            out
        }
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
        Step::Extract(e) => chosen(&[e.selector.is_some(), e.between.is_some(), e.all_text]),
        Step::Assert(a) => chosen(&[
            a.page_contains.is_some(),
            a.not_empty.is_some(),
            a.count_min.is_some(),
            a.url_contains.is_some(),
        ]),
        Step::Open(_) | Step::Scroll(_) => 1,
    };
    if one_of != 1 {
        out.push(v(&at, "one_of"));
    }
    match step {
        Step::Wait(w) if w.ms.or(w.timeout_ms).is_some_and(|ms| ms > WAIT_MAX_MS) => {
            out.push(v(&at, format!("max:{WAIT_MAX_MS}")));
        }
        Step::Extract(_) | Step::FollowPdf(_) => names.extend(step.output_name(i)),
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
pub const ACTIONS: [&str; 7] = [
    "open",
    "click",
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
}

/// What a run brought back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    fn la_strat() -> Value {
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
        let r = validate(&la_strat()).unwrap();
        assert_eq!(r.steps.len(), 6);
        assert!(r.steps[1].optional());
        assert_eq!(r.steps[4].label(), "Programme … Modalités");
    }

    #[test]
    fn captures_retention_defaults_and_round_trips() {
        let r = validate(&la_strat()).unwrap();
        assert_eq!(r.capture_retention(), CAPTURES_DEFAULT);
        let mut d = la_strat();
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
        let mut d = la_strat();
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
        let mut d = la_strat();
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
        let y = to_yaml(&validate(&la_strat()).unwrap());
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
