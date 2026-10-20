//! Server browser executor (site-recipes.md D325): plays a recipe in a
//! `chrome-headless-shell` reached over CDP (`PNEX_BROWSER_URL`, its own
//! container), one fresh browser context per run, one screenshot per step
//! with the target outlined.
//!
//! Each `open` is checked first by the guarded HTTP client (egress policy
//! R8, `robots.txt`, 2 s floor per page, redirects kept on the site, HTTP
//! status), and every navigation must stay on the site. On engines that
//! support it (Chrome), CDP `Fetch` interception also judges every request
//! of the page; Obscura ignores it and refuses internal addresses itself.
//! PDFs are fetched by the guarded HTTP client, from the site or an
//! `allow_hosts` host.
//!
//! Strict: a target not found or a false assertion stops the run at that
//! step; only `optional` steps may be missing.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use loco_rs::app::AppContext;
use pnex_core::browser_agent::fold;
use pnex_core::site_recipe::{
    fill, reason, site_base, site_host, AssertSpec, ExtractKind, ExtractSpec, PdfSpec, Recipe,
    RunExtract, RunStatus, RunStep, Step, StepStatus, Target, WaitSpec,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::cdp::Tab;
use crate::services::crawl::robots::Robots;

/// Longest step.
const STEP_TIMEOUT: Duration = Duration::from_secs(90);
/// Longest page load after an action.
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// Time a target may take to appear before it counts as missing.
const FIND_GRACE: Duration = Duration::from_secs(3);
/// Longest run.
const RUN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Pages of the site fetched at most every…
const HOST_DELAY: Duration = Duration::from_secs(2);
/// Largest PDF fetched, bytes.
const PDF_MAX: usize = 20 * 1024 * 1024;
/// Text kept per extract, chars.
const EXTRACT_MAX: usize = 200_000;
/// Viewport of the screenshots.
const VIEW_W: i64 = 1280;
const VIEW_H: i64 = 900;

fn instance() -> String {
    crate::app::prod_host()
        .map(|h| format!("https://{h}"))
        .unwrap_or_else(|| "https://pnex.io".into())
}

fn user_agent_suffix() -> String {
    format!(
        "PNEX-Crawler/{} (+{})",
        env!("CARGO_PKG_VERSION"),
        instance()
    )
}

fn http() -> &'static reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        pnex_core::egress::guarded(reqwest::Client::builder())
            .redirect(pnex_core::egress::redirect_policy(5))
            .timeout(Duration::from_secs(60))
            .user_agent(user_agent_suffix())
            .build()
            .expect("reqwest client")
    })
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Same site: the recipe's host, with or without `www.`.
fn on_site(site: &str, url: &str) -> bool {
    site_host(url).as_deref() == Some(site)
}

/// Is `host` allowed by the egress policy, its resolved addresses included.
pub(super) async fn egress_ok(host: &str) -> bool {
    let policy = pnex_core::egress::policy();
    if pnex_core::egress::check_host(host, policy).is_some() {
        return false;
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if bare.parse::<std::net::IpAddr>().is_ok() || policy == pnex_core::egress::EgressPolicy::Open {
        return true;
    }
    match tokio::net::lookup_host((bare, 443)).await {
        // shortcut: Chrome resolves again on its own (rebinding window); the
        // browser container's network policy is the hard guard in production.
        Ok(addrs) => !pnex_core::egress::filter_addrs(addrs, policy).is_empty(),
        Err(_) => true,
    }
}

/// Shared between the request interceptor and the steps.
#[derive(Default)]
struct Guard {
    /// Last top-level page refused: (reason token, address).
    blocked: Option<(&'static str, String)>,
    /// Hosts already judged.
    hosts: HashMap<String, bool>,
    last_page: Option<Instant>,
    /// HTTP status of the last top-level page received.
    status: Option<i64>,
}

/// A failed step: reason token and diagnostic as is.
struct Fail(&'static str, Option<String>);

impl Fail {
    fn of(token: &'static str) -> Self {
        Fail(token, None)
    }
}

type StepResult = Result<StepOk, Fail>;

#[derive(Default)]
struct StepOk {
    /// Optional step whose target was missing.
    skipped: bool,
}

struct Run {
    ctx: AppContext,
    org_id: i64,
    run_id: Uuid,
    recipe: Recipe,
    params: BTreeMap<String, String>,
    site: String,
    base: String,
    allow: Vec<String>,
    page: Tab,
    guard: Arc<Mutex<Guard>>,
    robots: Arc<Robots>,
    store: Option<crate::services::artifact_store::S3Store>,
    steps: Vec<RunStep>,
    extracts: Vec<RunExtract>,
    keys: BTreeMap<String, String>,
    /// Capture taken by the step itself (click: before acting).
    pre_shot: Option<String>,
}

/// Plays a run in the background; the journal row is updated after each
/// step and at the end.
pub fn spawn(
    ctx: AppContext,
    org_id: i64,
    run_id: Uuid,
    recipe: Recipe,
    params: BTreeMap<String, String>,
) {
    tokio::spawn(async move {
        let db = ctx.db.clone();
        let outcome = tokio::time::timeout(RUN_TIMEOUT, play(ctx, org_id, run_id, recipe, params))
            .await
            .unwrap_or(Err((None, reason::TIMEOUT)));
        let (status, step, why) = match outcome {
            Ok(()) => (RunStatus::Ok, None, None),
            Err((step, why)) => (RunStatus::Failed, step, Some(why)),
        };
        if let Err(e) = super::finish(&db, run_id, status, step, why).await {
            tracing::error!(error = %e, run = %run_id, "site recipe run not finished");
        }
    });
}

/// Longest setup call to the browser (context, page, interception).
const SETUP_TIMEOUT: Duration = Duration::from_secs(20);

/// A setup call, bounded and logged: a browser that stops answering fails
/// the run instead of holding it `running`.
async fn setup<T, E: std::fmt::Display>(
    stage: &'static str,
    call: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, (Option<u32>, &'static str)> {
    match tokio::time::timeout(SETUP_TIMEOUT, call).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => {
            tracing::warn!(stage, error = %e, "site recipe browser setup failed");
            Err((None, reason::BROWSER))
        }
        Err(_) => {
            tracing::warn!(stage, "site recipe browser setup timed out");
            Err((None, reason::BROWSER))
        }
    }
}

/// `Err((failed step, reason))`.
async fn play(
    ctx: AppContext,
    org_id: i64,
    run_id: Uuid,
    recipe: Recipe,
    params: BTreeMap<String, String>,
) -> Result<(), (Option<u32>, &'static str)> {
    let site = site_host(&recipe.site).unwrap_or_default();
    let base = site_base(&recipe.site).unwrap_or_default();
    let ring = crate::services::secrets::Keyring::from_config(&ctx.config).ok();
    let target =
        super::endpoints::resolve(&ctx.db, ring.as_ref(), org_id, recipe.browser.as_deref())
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "browser endpoint not resolved");
                (None, reason::BROWSER)
            })?;
    let _slot = super::endpoints::slot(&target).await;
    let (cdp, _) = super::endpoints::connect(&target).await.map_err(|e| {
        tracing::warn!(error = %e, "browser endpoint unreachable");
        (None, reason::BROWSER)
    })?;
    let page = setup("tab", Tab::open(cdp.clone())).await?;
    let result = async {
        let robots = Arc::new(fetch_robots(&base).await);
        let guard = Arc::new(Mutex::new(Guard::default()));
        setup(
            "interception",
            intercept(&page, &site, guard.clone(), robots.clone()),
        )
        .await?;
        setup(
            "viewport",
            page.call(
                "Emulation.setDeviceMetricsOverride",
                json!({ "width": VIEW_W, "height": VIEW_H, "deviceScaleFactor": 1, "mobile": false }),
            ),
        )
        .await?;
        let version = setup("user agent", cdp.call(None, "Browser.getVersion", json!({}))).await?;
        let ua = version["userAgent"].as_str().unwrap_or_default();
        setup(
            "user agent",
            page.call(
                "Emulation.setUserAgentOverride",
                json!({ "userAgent": format!(
                    "{} {}",
                    ua.replace("HeadlessChrome", "Chrome"),
                    user_agent_suffix()
                ) }),
            ),
        )
        .await?;
        Ok::<_, (Option<u32>, &'static str)>((robots, guard))
    }
    .await;
    let (robots, guard) = match result {
        Ok(v) => v,
        Err(e) => {
            page.close().await;
            return Err(e);
        }
    };
    let allow = recipe
        .allow_hosts
        .iter()
        .filter_map(|h| site_host(h))
        .collect();
    let mut run = Run {
        store: crate::services::artifact_store::S3Config::from_config(&ctx.config)
            .store()
            .ok(),
        ctx,
        org_id,
        run_id,
        recipe,
        params,
        site,
        base,
        allow,
        page,
        guard,
        robots,
        steps: Vec::new(),
        extracts: Vec::new(),
        keys: BTreeMap::new(),
        pre_shot: None,
    };
    let result = run.all_steps().await;
    run.page.close().await;
    result
}

async fn fetch_robots(base: &str) -> Robots {
    let Ok(res) = http().get(format!("{base}/robots.txt")).send().await else {
        return Robots::default();
    };
    if !res.status().is_success() {
        return Robots::default();
    }
    Robots::parse(&res.text().await.unwrap_or_default())
}

/// Request guard of a page outside a run (endpoint test): egress policy,
/// pages of `site` only.
pub(super) async fn guard_page(page: &Tab, site: &str) -> Result<(), String> {
    intercept(
        page,
        site,
        Arc::new(Mutex::new(Guard::default())),
        Arc::new(Robots::default()),
    )
    .await
}

/// Judges every request of the page (see the module doc). Engines that
/// ignore `Fetch.enable` (Obscura) simply never pause a request.
async fn intercept(
    page: &Tab,
    site: &str,
    guard: Arc<Mutex<Guard>>,
    robots: Arc<Robots>,
) -> Result<(), String> {
    let mut events = page.cdp.subscribe();
    page.call(
        "Fetch.enable",
        json!({ "patterns": [
            { "urlPattern": "*" },
            // Pages paused again on their answer: their HTTP status.
            { "urlPattern": "*", "resourceType": "Document", "requestStage": "Response" }
        ] }),
    )
    .await?;
    let cdp = page.cdp.clone();
    let session = page.session.clone();
    let site = site.to_string();
    tokio::spawn(async move {
        loop {
            let ev = match events.recv().await {
                Ok(ev) => ev,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            if ev.method != "Fetch.requestPaused" || ev.session.as_deref() != Some(session.as_str())
            {
                continue;
            }
            let id = ev.params["requestId"].clone();
            if let Some(code) = ev.params["responseStatusCode"].as_i64() {
                guard.lock().expect("guard").status = Some(code);
                let _ = cdp
                    .call(
                        Some(&session),
                        "Fetch.continueRequest",
                        json!({ "requestId": id }),
                    )
                    .await;
                continue;
            }
            let url = ev.params["request"]["url"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let document = ev.params["resourceType"].as_str() == Some("Document");
            let refused = judge(&url, document, &site, &guard, &robots).await;
            let sent = match refused {
                None => {
                    cdp.call(
                        Some(&session),
                        "Fetch.continueRequest",
                        json!({ "requestId": id }),
                    )
                    .await
                }
                Some(_) => {
                    cdp.call(
                        Some(&session),
                        "Fetch.failRequest",
                        json!({ "requestId": id, "errorReason": "BlockedByClient" }),
                    )
                    .await
                }
            };
            if sent.is_err() {
                break;
            }
        }
    });
    Ok(())
}

/// `Some(reason)` when the request must not leave.
async fn judge(
    url: &str,
    document: bool,
    site: &str,
    guard: &Mutex<Guard>,
    robots: &Robots,
) -> Option<&'static str> {
    let Ok(u) = reqwest::Url::parse(url) else {
        return Some(reason::EGRESS);
    };
    if !matches!(u.scheme(), "http" | "https") {
        return (!matches!(u.scheme(), "data" | "blob" | "about")).then_some(reason::EGRESS);
    }
    let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
    let known = guard.lock().expect("guard").hosts.get(&host).copied();
    let allowed = match known {
        Some(ok) => ok,
        None => {
            let ok = egress_ok(&host).await;
            guard.lock().expect("guard").hosts.insert(host.clone(), ok);
            ok
        }
    };
    let refuse = |why: &'static str| {
        if document {
            guard.lock().expect("guard").blocked = Some((why, url.to_string()));
        }
        Some(why)
    };
    if !allowed {
        return refuse(reason::EGRESS);
    }
    if !document {
        return None;
    }
    if !on_site(site, url) {
        return refuse(reason::OFF_SITE);
    }
    if !robots.allows(u.path()) {
        return refuse(reason::ROBOTS);
    }
    // Pages of the site 2 s apart at least (D307 floor).
    let wait = {
        let mut g = guard.lock().expect("guard");
        let now = Instant::now();
        let at = g.last_page.map_or(now, |t| (t + HOST_DELAY).max(now));
        g.last_page = Some(at);
        at - now
    };
    tokio::time::sleep(wait).await;
    None
}

// ───────────────────────────── page helpers ─────────────────────────────

/// Finds the target, outlines it and brings it into view. Text matches
/// ignore case, accents and spacing; the innermost visible match wins.
const FIND_JS: &str = r#"(function (spec) {
  const norm = s => (s || '').normalize('NFD').replace(/[̀-ͯ]/g, '').replace(/\s+/g, ' ').trim().toLowerCase();
  // What a person can see: a box, not hidden, and no transparent ancestor
  // (closed menus are often opacity 0).
  const visible = e => { const r = e.getBoundingClientRect(); const s = getComputedStyle(e);
    if (!(r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none')) return false;
    for (let a = e; a && a.nodeType === 1; a = a.parentElement) { if (parseFloat(getComputedStyle(a).opacity) === 0) return false; }
    return true; };
  document.querySelectorAll('[data-pnex-target]').forEach(e => { e.style.removeProperty('outline'); e.style.removeProperty('box-shadow'); e.removeAttribute('data-pnex-target'); });
  let c = [];
  try {
    if (spec.selector) c = [...document.querySelectorAll(spec.selector)];
    else if (spec.role) {
      c = [...document.querySelectorAll('[role="' + spec.role.replace(/"/g, '') + '"]' + (spec.role === 'button' ? ',button' : spec.role === 'link' ? ',a[href]' : spec.role === 'tab' ? ',[role=tab]' : ''))];
      if (spec.name) c = c.filter(e => norm(e.getAttribute('aria-label') || e.innerText).includes(norm(spec.name)));
    } else if (spec.href_contains || spec.link_text_contains) {
      c = [...document.querySelectorAll('a[href]')].filter(a => spec.href_contains
        ? a.href.toLowerCase().includes(spec.href_contains.toLowerCase())
        : norm(a.innerText).includes(norm(spec.link_text_contains)));
    } else {
      const want = norm(spec.text || spec.text_contains);
      c = [...document.querySelectorAll('body *')].filter(e => {
        const t = norm(e.innerText); return spec.text ? t === want : t.includes(want); });
      c = c.filter(e => !c.some(o => o !== e && e.contains(o)));
    }
  } catch (err) { return { found: 0, error: String(err) }; }
  c = c.filter(visible);
  if (!c.length) return { found: 0 };
  const el = c[0];
  el.scrollIntoView({ block: 'center' });
  el.setAttribute('data-pnex-target', '1');
  el.style.setProperty('outline', '4px solid #e11d48', 'important');
  el.style.setProperty('box-shadow', '0 0 0 8px rgba(225, 29, 72, 0.25)', 'important');
  return { found: c.length, href: el.closest('a[href]') ? el.closest('a[href]').href : null };
})"#;

const CLICK_JS: &str = r#"(function () {
  const el = document.querySelector('[data-pnex-target]');
  if (!el) return false;
  el.style.removeProperty('outline');
  el.style.removeProperty('box-shadow');
  el.removeAttribute('data-pnex-target');
  const target = el.closest('a,button,[role=button],[role=tab],[role=link],summary,label') || el;
  // Clicked after this call returns: a navigation must not swallow its reply.
  setTimeout(() => target.click(), 0);
  return true;
})()"#;

impl Run {
    async fn eval<T: serde::de::DeserializeOwned>(&self, js: String) -> Option<T> {
        self.eval_within(js, Duration::from_secs(10)).await
    }

    /// An evaluation sent while some engines (Obscura) load a page gets no
    /// answer: never wait for it longer than `limit`.
    async fn eval_within<T: serde::de::DeserializeOwned>(
        &self,
        js: String,
        limit: Duration,
    ) -> Option<T> {
        let v = self.page.eval(&js, limit).await.ok()?;
        serde_json::from_value(v).ok()
    }

    /// Finds a target, giving late elements (cookie banners, scripts that
    /// render after load) up to [`FIND_GRACE`] to appear.
    async fn find(&self, spec: serde_json::Value) -> (u64, Option<String>) {
        let until = Instant::now() + FIND_GRACE;
        loop {
            let out: serde_json::Value = self
                .eval(format!("{FIND_JS}({spec})"))
                .await
                .unwrap_or_default();
            let found = out["found"].as_u64().unwrap_or(0);
            if found > 0 || Instant::now() >= until {
                return (found, out["href"].as_str().map(str::to_string));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn text(&self) -> String {
        self.eval("document.body ? document.body.innerText : ''".into())
            .await
            .unwrap_or_default()
    }

    async fn url(&self) -> String {
        self.page.url().await
    }

    /// Waits for the page after an action that may navigate: it is ready
    /// once it answers with a `readyState` past `loading`. Some engines
    /// (Obscura) do not answer at all while they load a page: silence means
    /// loading, and a click that does not navigate answers at once.
    async fn wait_loaded(&self) {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let until = Instant::now() + LOAD_TIMEOUT;
        while Instant::now() < until {
            let state: Option<String> = self
                .eval_within("document.readyState".into(), Duration::from_secs(2))
                .await;
            if state.is_some_and(|s| s != "loading") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    /// A page refused by the interceptor, or an HTTP error of the page.
    async fn page_failure(&self) -> Option<Fail> {
        let (blocked, status) = {
            let mut g = self.guard.lock().expect("guard");
            (g.blocked.take(), g.status.take())
        };
        if let Some((why, url)) = blocked {
            return Some(Fail(why, Some(url)));
        }
        match status.unwrap_or(0) {
            429 => Some(Fail(reason::RATE_LIMITED, Some("HTTP 429".into()))),
            s if s >= 400 => Some(Fail(reason::HTTP_STATUS, Some(format!("HTTP {s}")))),
            _ => None,
        }
    }

    fn fill(&self, s: &str) -> String {
        fill(s, &self.params)
    }

    fn target_spec(&self, t: &Target) -> serde_json::Value {
        let f = |o: &Option<String>| o.as_deref().map(|s| self.fill(s));
        json!({
            "text": f(&t.text),
            "text_contains": f(&t.text_contains),
            "selector": f(&t.selector),
            "role": f(&t.role),
            "name": f(&t.name),
        })
    }

    async fn screenshot(&mut self, index: usize) -> Option<String> {
        let shot = self.page.screenshot().await.ok()?;
        let file = format!("step-{index}.jpg");
        self.put(&file, shot).await.then_some(file)
    }

    async fn put(&mut self, file: &str, bytes: Vec<u8>) -> bool {
        let Some(store) = &self.store else {
            return false;
        };
        // shortcut: run files are never purged yet; apply the mission
        // retention (D268) when runs come from scheduled flows (N2).
        let key = format!("org_{}/site_recipes/{}/{file}", self.org_id, self.run_id);
        match store.put_bytes(&key, Bytes::from(bytes)).await {
            Ok(()) => {
                self.keys.insert(file.to_string(), key);
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "site recipe file not stored");
                false
            }
        }
    }

    async fn all_steps(&mut self) -> Result<(), (Option<u32>, &'static str)> {
        for p in &self.recipe.params {
            if !self.params.contains_key(p) {
                return Err((None, reason::PARAM_MISSING));
            }
        }
        let steps = self.recipe.steps.clone();
        for (i, step) in steps.iter().enumerate() {
            let started = Instant::now();
            let outcome = tokio::time::timeout(STEP_TIMEOUT, self.step(i, step))
                .await
                .unwrap_or(Err(Fail::of(reason::TIMEOUT)));
            let shot = match self.pre_shot.take() {
                Some(s) => Some(s),
                None => self.screenshot(i).await,
            };
            let (status, why, detail) = match &outcome {
                Ok(o) if o.skipped => (StepStatus::Skipped, Some(reason::TARGET_NOT_FOUND), None),
                Ok(_) => (StepStatus::Ok, None, None),
                Err(Fail(w, d)) => (StepStatus::Failed, Some(*w), d.clone()),
            };
            self.steps.push(RunStep {
                index: i as u32,
                action: step.action().into(),
                label: self.fill(&step.label()),
                status,
                reason: why.map(str::to_string),
                detail,
                url: self.url().await,
                ms: started.elapsed().as_millis() as u64,
                screenshot: shot,
            });
            if let Err(e) = super::save_progress(
                &self.ctx.db,
                self.run_id,
                &self.steps,
                &self.extracts,
                &self.keys,
            )
            .await
            {
                tracing::warn!(error = %e, "site recipe progress not saved");
            }
            if let Err(Fail(w, _)) = outcome {
                return Err((Some(i as u32), w));
            }
        }
        Ok(())
    }

    async fn step(&mut self, i: usize, step: &Step) -> StepResult {
        match step {
            Step::Open(path) => self.open(path).await,
            Step::Click(t) => self.click(t).await,
            Step::Scroll(to) => self.scroll(to).await,
            Step::Wait(w) => self.wait(w).await,
            Step::FollowPdf(p) => self.follow_pdf(i, p).await,
            Step::Extract(e) => self.extract(i, e).await,
            Step::Assert(a) => self.assert(a).await,
        }
    }

    async fn open(&mut self, path: &str) -> StepResult {
        let path = self.fill(path);
        let Ok(url) = reqwest::Url::parse(&self.base).and_then(|b| b.join(&path)) else {
            return Err(Fail(reason::OFF_SITE, Some(path)));
        };
        if !on_site(&self.site, url.as_str()) {
            return Err(Fail(reason::OFF_SITE, Some(url.to_string())));
        }
        self.preflight(&url).await?;
        self.guard.lock().expect("guard").status = None;
        // Navigation through the page, not `Page.navigate`: some engines
        // waits for a load event some engines (Obscura) never send.
        let target = serde_json::to_string(url.as_str()).unwrap_or_default();
        let _: Option<bool> = self
            .eval(format!(
                "(setTimeout(() => {{ location.href = {target}; }}, 0), true)"
            ))
            .await;
        self.wait_loaded().await;
        if let Some(f) = self.page_failure().await {
            return Err(f);
        }
        self.still_on_site().await
    }

    /// Checks a page before the browser opens it, with the guarded client:
    /// egress policy, `robots.txt`, the 2 s floor, redirects kept on the
    /// site, and its HTTP status. Engines that do not report statuses nor
    /// let requests be intercepted over CDP (Obscura) rely on this.
    async fn preflight(&self, url: &reqwest::Url) -> Result<(), Fail> {
        if !self.robots.allows(url.path()) {
            return Err(Fail(reason::ROBOTS, Some(url.to_string())));
        }
        let wait = {
            let mut g = self.guard.lock().expect("guard");
            let now = Instant::now();
            let at = g.last_page.map_or(now, |t| (t + HOST_DELAY).max(now));
            g.last_page = Some(at);
            at - now
        };
        tokio::time::sleep(wait).await;
        let res = http()
            .get(url.clone())
            .send()
            .await
            .map_err(|e| Fail(reason::EGRESS, Some(e.to_string())))?;
        if !on_site(&self.site, res.url().as_str()) {
            return Err(Fail(reason::OFF_SITE, Some(res.url().to_string())));
        }
        match res.status().as_u16() {
            429 => Err(Fail(reason::RATE_LIMITED, Some("HTTP 429".into()))),
            s if s >= 400 => Err(Fail(reason::HTTP_STATUS, Some(format!("HTTP {s}")))),
            _ => Ok(()),
        }
    }

    /// A navigation that left the site stops the run.
    async fn still_on_site(&self) -> StepResult {
        let now = self.url().await;
        if now.is_empty() || now.starts_with("about:") || on_site(&self.site, &now) {
            Ok(StepOk::default())
        } else {
            Err(Fail(reason::OFF_SITE, Some(now)))
        }
    }

    async fn click(&mut self, t: &Target) -> StepResult {
        let (found, _) = self.find(self.target_spec(t)).await;
        if found == 0 {
            return if t.optional {
                Ok(StepOk { skipped: true })
            } else {
                Err(Fail::of(reason::TARGET_NOT_FOUND))
            };
        }
        // The step keeps the capture of the outlined target: where it clicks.
        self.pre_shot = self.screenshot(self.steps.len()).await;
        self.guard.lock().expect("guard").status = None;
        let _: Option<bool> = self.eval(CLICK_JS.into()).await;
        self.wait_loaded().await;
        if let Some(f) = self.page_failure().await {
            return Err(f);
        }
        // shortcut: the status of a page reached by a click is unknown on
        // engines without CDP interception (Obscura); the next check fails
        // instead. Preflight the new address if that proves too vague.
        self.still_on_site().await?;
        if let Ok(u) = reqwest::Url::parse(&self.url().await) {
            if !self.robots.allows(u.path()) {
                return Err(Fail(reason::ROBOTS, Some(u.to_string())));
            }
        }
        Ok(StepOk::default())
    }

    async fn scroll(&mut self, to: &str) -> StepResult {
        let to = self.fill(to);
        if to.trim() == "bottom" {
            let _: Option<bool> = self
                .eval("(window.scrollTo(0, document.body.scrollHeight), true)".into())
                .await;
        } else if self.find(json!({ "selector": to })).await.0 == 0 {
            return Err(Fail::of(reason::TARGET_NOT_FOUND));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(StepOk::default())
    }

    async fn wait(&mut self, w: &WaitSpec) -> StepResult {
        if let Some(ms) = w.ms {
            tokio::time::sleep(Duration::from_millis(ms.into())).await;
            return Ok(StepOk::default());
        }
        let want = fold(&self.fill(w.text.as_deref().unwrap_or_default()));
        let until = Instant::now() + Duration::from_millis(w.timeout_ms.unwrap_or(10_000).into());
        while Instant::now() < until {
            if fold(&self.text().await).contains(&want) {
                return Ok(StepOk::default());
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        if w.optional {
            Ok(StepOk { skipped: true })
        } else {
            Err(Fail::of(reason::TIMEOUT))
        }
    }

    async fn follow_pdf(&mut self, i: usize, p: &PdfSpec) -> StepResult {
        let spec = json!({
            "link_text_contains": p.link_text_contains.as_deref().map(|s| self.fill(s)),
            "href_contains": p.href_contains.as_deref().map(|s| self.fill(s)),
        });
        let (found, href) = self.find(spec).await;
        let Some(href) = href.filter(|_| found > 0) else {
            return if p.optional {
                Ok(StepOk { skipped: true })
            } else {
                Err(Fail::of(reason::TARGET_NOT_FOUND))
            };
        };
        let host = site_host(&href).unwrap_or_default();
        if host != self.site && !self.allow.contains(&host) {
            return Err(Fail(reason::OFF_SITE, Some(href)));
        }
        let Ok(u) = reqwest::Url::parse(&href) else {
            return Err(Fail(reason::OFF_SITE, Some(href)));
        };
        if host == self.site && !self.robots.allows(u.path()) {
            return Err(Fail(reason::ROBOTS, Some(href)));
        }
        let res = http()
            .get(u.clone())
            .send()
            .await
            .map_err(|e| Fail(reason::EGRESS, Some(e.to_string())))?;
        let status = res.status().as_u16();
        if status == 429 {
            return Err(Fail(reason::RATE_LIMITED, Some("HTTP 429".into())));
        }
        if status >= 400 {
            return Err(Fail(reason::HTTP_STATUS, Some(format!("HTTP {status}"))));
        }
        let final_host = site_host(res.url().as_str()).unwrap_or_default();
        if final_host != self.site && !self.allow.contains(&final_host) {
            return Err(Fail(reason::OFF_SITE, Some(res.url().to_string())));
        }
        if res.content_length().is_some_and(|n| n as usize > PDF_MAX) {
            return Err(Fail::of(reason::TOO_LARGE));
        }
        let mut res = res;
        let mut body = Vec::new();
        while let Some(chunk) = res
            .chunk()
            .await
            .map_err(|e| Fail(reason::HTTP_STATUS, Some(e.to_string())))?
        {
            if body.len() + chunk.len() > PDF_MAX {
                return Err(Fail::of(reason::TOO_LARGE));
            }
            body.extend_from_slice(&chunk);
        }
        if !body.starts_with(b"%PDF") {
            return Err(Fail(reason::NOT_A_PDF, Some(href)));
        }
        let sha = sha256_hex(&body);
        let bytes = body.len() as u64;
        let text = pdf_text(body.clone()).await;
        let file = format!("pdf-{i}.pdf");
        let stored = self.put(&file, body).await;
        let name = p.name.clone().unwrap_or_else(|| format!("pdf-{i}"));
        self.push_extract(RunExtract {
            name,
            kind: ExtractKind::Pdf,
            chars: text.chars().count() as u64,
            text: text.chars().take(EXTRACT_MAX).collect(),
            sha256: sha,
            from: Some(href),
            file: stored.then_some(file),
            bytes: Some(bytes),
        });
        Ok(StepOk::default())
    }

    fn push_extract(&mut self, e: RunExtract) {
        self.extracts.retain(|o| o.name != e.name);
        self.extracts.push(e);
    }

    async fn extract(&mut self, i: usize, e: &ExtractSpec) -> StepResult {
        let text = if let Some(sel) = &e.selector {
            let (found, _) = self.find(json!({ "selector": self.fill(sel) })).await;
            if found == 0 {
                return if e.optional {
                    Ok(StepOk { skipped: true })
                } else {
                    Err(Fail::of(reason::TARGET_NOT_FOUND))
                };
            }
            let sel = serde_json::to_string(&self.fill(sel)).unwrap_or_default();
            self.eval::<String>(format!(
                "[...document.querySelectorAll({sel})].map(e => e.innerText.trim()).filter(Boolean).join('\\n\\n')"
            ))
            .await
            .unwrap_or_default()
        } else if let Some([a, b]) = &e.between {
            match between(&self.text().await, &self.fill(a), &self.fill(b)) {
                Some(t) => t,
                None if e.optional => return Ok(StepOk { skipped: true }),
                None => return Err(Fail::of(reason::TARGET_NOT_FOUND)),
            }
        } else {
            self.text().await
        };
        let name = e.name.clone().unwrap_or_else(|| format!("extract-{i}"));
        self.push_extract(RunExtract {
            name,
            kind: ExtractKind::Text,
            chars: text.chars().count() as u64,
            sha256: sha256_hex(text.as_bytes()),
            text: text.chars().take(EXTRACT_MAX).collect(),
            from: Some(self.url().await),
            file: None,
            bytes: None,
        });
        Ok(StepOk::default())
    }

    async fn assert(&mut self, a: &AssertSpec) -> StepResult {
        let ok = if let Some(t) = &a.page_contains {
            fold(&self.text().await).contains(&fold(&self.fill(t)))
        } else if let Some(n) = &a.not_empty {
            self.extracts
                .iter()
                .any(|e| &e.name == n && !e.text.trim().is_empty())
        } else if let Some(c) = &a.count_min {
            let sel = serde_json::to_string(&self.fill(&c.selector)).unwrap_or_default();
            let n: u64 = self
                .eval(format!("document.querySelectorAll({sel}).length"))
                .await
                .unwrap_or(0);
            n >= u64::from(c.min)
        } else if let Some(u) = &a.url_contains {
            self.url().await.contains(&self.fill(u))
        } else {
            false
        };
        if ok {
            Ok(StepOk::default())
        } else {
            Err(Fail::of(reason::ASSERT_FAILED))
        }
    }
}

/// Text of a PDF (empty when unreadable or scanned).
async fn pdf_text(bytes: Vec<u8>) -> String {
    use pnex_core::doc_extract::{self, DocFormat, Limits};
    let job = tokio::task::spawn_blocking(move || {
        doc_extract::extract(DocFormat::Pdf, &bytes, &Limits::default())
    });
    match tokio::time::timeout(Duration::from_secs(60), job).await {
        Ok(Ok(Ok(doc))) => doc
            .sections
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    }
}

/// Text between the first `a` and the next `b` after it (case and accents
/// ignored); `None` when `a` is missing. Without `b`, up to the end.
fn between(text: &str, a: &str, b: &str) -> Option<String> {
    // Fold char by char so folded and original positions line up.
    let chars: Vec<char> = text.chars().collect();
    let folded: Vec<char> = chars
        .iter()
        .map(|c| fold(&c.to_string()).chars().next().unwrap_or(*c))
        .collect();
    let find = |needle: &str, from: usize| -> Option<usize> {
        let n: Vec<char> = fold(needle).chars().collect();
        if n.is_empty() || folded.len() < n.len() {
            return None;
        }
        (from..=folded.len() - n.len()).find(|&i| folded[i..i + n.len()] == n[..])
    };
    let start = find(a, 0)? + a.chars().count();
    let end = find(b, start).unwrap_or(chars.len());
    Some(
        chars[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Live probe of a CDP endpoint: `PNEX_BROWSER_URL=http://127.0.0.1:9222
    /// cargo test -p pnex-backend browser_probe -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs a running browser"]
    async fn browser_probe() {
        let target = super::super::endpoints::resolve_platform_for_tests();
        let report = super::super::endpoints::test(&target).await;
        eprintln!("probe: {report:?}");
        assert!(report.ok, "endpoint test failed");
        let (cdp, _) = super::super::endpoints::connect(&target)
            .await
            .expect("connect");
        let tab = Tab::open(cdp).await.expect("tab");
        let guard = Arc::new(Mutex::new(Guard::default()));
        intercept(
            &tab,
            "example.com",
            guard.clone(),
            Arc::new(Robots::default()),
        )
        .await
        .expect("interception");
        tab.eval(
            "(location.href = 'https://example.com/missing-404', true)",
            Duration::from_secs(5),
        )
        .await
        .expect("eval");
        tokio::time::sleep(Duration::from_secs(3)).await;
        eprintln!(
            "probe: url {} status {:?}",
            tab.url().await,
            guard.lock().unwrap().status
        );
        let text = tab
            .eval("document.body.innerText", Duration::from_secs(5))
            .await
            .expect("text");
        eprintln!(
            "probe: text {:?}",
            text.as_str()
                .map(|t| t.chars().take(50).collect::<String>())
        );
        assert!(!tab.screenshot().await.expect("screenshot").is_empty());
        tab.close().await;
    }

    #[test]
    fn only_the_recipe_site_is_on_site() {
        assert!(on_site("la-strat.fr", "https://www.la-strat.fr/formations"));
        assert!(!on_site("la-strat.fr", "https://evil.example/la-strat.fr"));
    }
}
