//! Browser side: exports for the options page and the service worker, and
//! the agent loop.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use pnex_core::browser_agent::{
    self as ba, AgentFingerprint, AgentMsg, AssignedMission, MissionSpec, MissionState,
    PairRequest, PairResponse, ServerMsg, TicketRequest,
};
use pnex_core::err_codes;
use pnex_core::frame::{Initiator, Link};
use serde::Deserialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::host::{self, err_text, text};
use crate::{
    due_missions, native_redirect, next_run, normalize_server, sso_url, ws_url, Creds, CREDS_KEY,
    PING_MS,
};

/// Longest wait of the user in the sign-in tab.
const LOGIN_TIMEOUT_MS: f64 = 300_000.0;

#[derive(Deserialize)]
struct HttpReply {
    status: u16,
    body: String,
}

async fn call(
    method: &str,
    url: &str,
    bearer: &str,
    org: &str,
    body: &str,
) -> Result<HttpReply, String> {
    call_with(method, url, bearer, org, body, "").await
}

/// [`call`] with more headers (`extra`: JSON object).
async fn call_with(
    method: &str,
    url: &str,
    bearer: &str,
    org: &str,
    body: &str,
    extra: &str,
) -> Result<HttpReply, String> {
    let raw = host::http(method, url, bearer, org, body, extra)
        .await
        .map_err(|e| err_text(&e))?;
    serde_json::from_str(&text(&raw)).map_err(|e| e.to_string())
}

/// Machine code of a PNeX error body, else the HTTP status.
fn error_of(reply: &HttpReply) -> String {
    serde_json::from_str::<serde_json::Value>(&reply.body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| format!("http-{}", reply.status))
}

fn js_err(msg: impl Into<String>) -> JsValue {
    JsValue::from_str(&msg.into())
}

// ───────────────────────── Options page ─────────────────────────

/// Server the zip was downloaded from (`server.json`), if any.
#[wasm_bindgen]
pub async fn server_hint() -> Option<String> {
    host::bundled_server().await.as_string()
}

/// Signs the user in through the server's native bridge (PKCE). Resolves
/// with JSON `{"access_token": "...", "orgs": [{"id", "name", "role"}]}`;
/// the token stays in the options page memory and dies with it.
#[wasm_bindgen]
pub async fn login(server: String) -> Result<String, JsValue> {
    let server = normalize_server(&server).ok_or_else(|| js_err("server-invalid"))?;
    let verifier = host::random_b64(32);
    let challenge = text(
        &host::sha256_b64(&verifier)
            .await
            .map_err(|e| js_err(err_text(&e)))?,
    );
    let state = host::random_b64(24);
    let tab = host::open_tab(&sso_url(&server, &challenge, &state))
        .await
        .map_err(|e| js_err(err_text(&e)))?
        .as_f64()
        .unwrap_or(-1.0);
    let started = host::now_ms();
    let code = loop {
        host::sleep(1500.0).await;
        if host::now_ms() - started > LOGIN_TIMEOUT_MS {
            return Err(js_err("login-timeout"));
        }
        let reply = call(
            "GET",
            &format!("{server}/api/v1/oauth2/native/{state}"),
            "",
            "",
            "",
        )
        .await
        .map_err(js_err)?;
        let code = serde_json::from_str::<serde_json::Value>(&reply.body)
            .ok()
            .and_then(|v| v["code"].as_str().map(str::to_string));
        if let Some(code) = code {
            break code;
        }
    };
    host::close_tab(tab).await;
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "code_verifier": verifier,
        "redirect_uri": native_redirect(&server),
    });
    let reply = call(
        "POST",
        &format!("{server}/api/v1/oauth2/token"),
        "",
        "",
        &body.to_string(),
    )
    .await
    .map_err(js_err)?;
    if reply.status != 200 {
        return Err(js_err(error_of(&reply)));
    }
    let tokens: pnex_core::TokenResponse =
        serde_json::from_str(&reply.body).map_err(|e| js_err(e.to_string()))?;
    let reply = call(
        "GET",
        &format!("{server}/api/v1/user-info"),
        &tokens.access_token,
        "",
        "",
    )
    .await
    .map_err(js_err)?;
    let info: serde_json::Value =
        serde_json::from_str(&reply.body).map_err(|e| js_err(e.to_string()))?;
    let orgs: Vec<serde_json::Value> = info["orgs"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| serde_json::json!({ "id": o["id"], "name": o["name"], "role": o["role"] }))
                .collect()
        })
        .unwrap_or_default();
    Ok(serde_json::json!({ "access_token": tokens.access_token, "orgs": orgs }).to_string())
}

/// Pairs this install with `org_id` (D266) and stores the agent
/// credentials. Resolves with the agent name.
#[wasm_bindgen]
pub async fn pair(server: String, access_token: String, org_id: f64) -> Result<String, JsValue> {
    let server = normalize_server(&server).ok_or_else(|| js_err("server-invalid"))?;
    let org_id = org_id as i64;
    let install_key = text(
        &host::install_key()
            .await
            .map_err(|e| js_err(err_text(&e)))?,
    );
    let ts = host::now_ms() as i64;
    let signature = text(
        &host::sign(&ba::pair_message(org_id, ts))
            .await
            .map_err(|e| js_err(err_text(&e)))?,
    );
    let fingerprint: AgentFingerprint =
        serde_json::from_str(&host::fingerprint()).unwrap_or_default();
    let body = serde_json::to_string(&PairRequest {
        install_key,
        fingerprint,
        ts,
        signature,
    })
    .map_err(|e| js_err(e.to_string()))?;
    let reply = call(
        "POST",
        &format!("{server}/api/v1/extension/agents"),
        &access_token,
        &org_id.to_string(),
        &body,
    )
    .await
    .map_err(js_err)?;
    if reply.status != 201 {
        return Err(js_err(error_of(&reply)));
    }
    let paired: PairResponse =
        serde_json::from_str(&reply.body).map_err(|e| js_err(e.to_string()))?;
    let creds = Creds {
        server,
        org_id,
        agent_id: paired.agent.id,
        device_id: paired.agent.device_id,
        name: paired.agent.name.clone(),
        token: paired.token,
        encryption_key: paired.encryption_key,
    };
    let json = serde_json::to_string(&creds).map_err(|e| js_err(e.to_string()))?;
    host::store_set(CREDS_KEY, &json)
        .await
        .map_err(|e| js_err(err_text(&e)))?;
    Ok(paired.agent.name)
}

/// Forgets the credentials (the agent stays listed in PNeX until removed
/// there).
#[wasm_bindgen]
pub async fn unpair() -> Result<(), JsValue> {
    host::store_remove(CREDS_KEY)
        .await
        .map(|_| ())
        .map_err(|e| js_err(err_text(&e)))
}

// ───────────────────────── Agent loop ─────────────────────────

thread_local! {
    static RUNNING: RefCell<bool> = const { RefCell::new(false) };
}

/// Starts the agent loop once per worker life (idempotent: every wake-up
/// of the service worker calls it).
#[wasm_bindgen]
pub fn start() {
    let first = RUNNING.with(|r| !std::mem::replace(&mut *r.borrow_mut(), true));
    if first {
        spawn_local(agent_loop());
    }
}

async fn load_creds() -> Option<Creds> {
    let raw = host::store_get(CREDS_KEY).await.ok()?;
    serde_json::from_str(&raw.as_string()?).ok()
}

#[derive(Default)]
struct Status {
    paired: bool,
    connected: bool,
    name: String,
    server: String,
    missions: Vec<serde_json::Value>,
    missing_origins: BTreeSet<String>,
}

impl Status {
    fn publish(&self) {
        host::set_status(
            &serde_json::json!({
                "paired": self.paired,
                "connected": self.connected,
                "name": self.name,
                "server": self.server,
                "missions": self.missions,
                "missing_origins": self.missing_origins,
            })
            .to_string(),
        );
    }
}

async fn agent_loop() {
    let mut backoff = 2_000.0;
    loop {
        let Some(creds) = load_creds().await else {
            Status::default().publish();
            host::sleep(10_000.0).await;
            continue;
        };
        match Session::open(&creds).await {
            Ok(session) => {
                backoff = 2_000.0;
                session.run().await;
            }
            Err(e) => {
                console_log(&format!("PNeX agent: connection failed ({e})"));
                let status = Status {
                    paired: true,
                    name: creds.name.clone(),
                    server: creds.server.clone(),
                    ..Default::default()
                };
                status.publish();
            }
        }
        host::sleep(backoff).await;
        backoff = (backoff * 2.0).min(60_000.0);
    }
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = log)]
    fn console_log(s: &str);
}

/// Link state shared with the websocket callbacks.
struct Shared {
    socket: f64,
    initiator: Option<Initiator>,
    link: Option<Link>,
    inbox: Vec<ServerMsg>,
    closed: bool,
}

struct Session {
    creds: Creds,
    shared: Rc<RefCell<Shared>>,
    _on_text: Closure<dyn FnMut(String)>,
    _on_close: Closure<dyn FnMut(u16)>,
}

impl Session {
    /// Signed ticket → websocket → Noise handshake (first frame).
    async fn open(creds: &Creds) -> Result<Self, String> {
        let ts = host::now_ms() as i64;
        let signature = text(
            &host::sign(&ba::ticket_message(&creds.device_id, ts))
                .await
                .map_err(|e| err_text(&e))?,
        );
        let body =
            serde_json::to_string(&TicketRequest { ts, signature }).map_err(|e| e.to_string())?;
        let reply = call(
            "POST",
            &format!("{}/api/v1/extension/ws-ticket", creds.server),
            &creds.token,
            "",
            &body,
        )
        .await?;
        if reply.status != 200 {
            return Err(error_of(&reply));
        }
        let ticket = serde_json::from_str::<serde_json::Value>(&reply.body)
            .ok()
            .and_then(|v| v["ticket"].as_str().map(str::to_string))
            .ok_or("ticket missing")?;
        let key = pnex_core::frame::decode_key(&creds.encryption_key).ok_or("bad key")?;
        let (initiator, msg1) = Initiator::start(&key, &creds.device_id);

        let shared = Rc::new(RefCell::new(Shared {
            socket: -1.0,
            initiator: Some(initiator),
            link: None,
            inbox: Vec::new(),
            closed: false,
        }));
        let s = shared.clone();
        let on_text = Closure::<dyn FnMut(String)>::new(move |raw: String| {
            let mut sh = s.borrow_mut();
            if let Some(init) = sh.initiator.take() {
                let link = pnex_core::frame::decode_handshake(&raw).and_then(|m| init.finish(&m));
                if link.is_none() {
                    sh.closed = true;
                }
                sh.link = link;
                return;
            }
            let plain = sh.link.as_mut().and_then(|l| l.open_text(&raw));
            match plain.and_then(|p| serde_json::from_str::<ServerMsg>(&p).ok()) {
                Some(msg) => sh.inbox.push(msg),
                // An unauthenticated frame desynchronizes the link: drop it.
                None => sh.closed = true,
            }
        });
        let s = shared.clone();
        let on_close = Closure::<dyn FnMut(u16)>::new(move |_code: u16| {
            s.borrow_mut().closed = true;
        });
        let socket = host::ws_open(&ws_url(&creds.server, &ticket), &on_text, &on_close)
            .await
            .map_err(|e| err_text(&e))?
            .as_f64()
            .ok_or("no socket")?;
        shared.borrow_mut().socket = socket;
        host::ws_send(socket, &pnex_core::frame::encode_handshake(&msg1));
        Ok(Self {
            creds: creds.clone(),
            shared,
            _on_text: on_text,
            _on_close: on_close,
        })
    }

    fn send(&self, msg: &AgentMsg) {
        let Ok(plain) = serde_json::to_string(msg) else {
            return;
        };
        let mut sh = self.shared.borrow_mut();
        let socket = sh.socket;
        if let Some(link) = sh.link.as_mut() {
            let wire = link.seal_text(&plain);
            host::ws_send(socket, &wire);
        }
    }

    async fn run(self) {
        let this = Rc::new(self);
        let plan = Rc::new(RefCell::new(Plan::default()));
        let mut status = Status {
            paired: true,
            name: this.creds.name.clone(),
            server: this.creds.server.clone(),
            ..Default::default()
        };
        let mut last_ping = host::now_ms();
        loop {
            host::sleep(500.0).await;
            if this.shared.borrow().closed {
                break;
            }
            status.connected = this.shared.borrow().link.is_some();
            let inbox = std::mem::take(&mut this.shared.borrow_mut().inbox);
            for msg in inbox {
                match msg {
                    ServerMsg::Missions { missions } => plan.borrow_mut().replace(missions),
                    ServerMsg::Capture {
                        mission_id,
                        platform_ids,
                        captions,
                    } => {
                        let mut p = plan.borrow_mut();
                        for id in captions {
                            if let Some(url) = p.post_urls.get(&id).cloned() {
                                p.caption_queue.push_back((mission_id.clone(), id, url));
                            }
                        }
                        for id in platform_ids {
                            if let Some(url) = p.videos.remove(&id) {
                                let page =
                                    p.post_urls.get(&id).cloned().unwrap_or_else(|| url.clone());
                                p.downloads.push_back((mission_id.clone(), id, url, page));
                            }
                        }
                    }
                    ServerMsg::Pong => {}
                }
            }
            // Captions: one page at a time in the platform's tab (D255, D258).
            let next_caption = {
                let mut p = plan.borrow_mut();
                let now = host::now_ms();
                let ready = p.caption_queue.front().is_some_and(|(_, _, url)| {
                    let platform = platform_of_url(url);
                    !p.busy.contains(platform)
                        && p.last_nav
                            .get(platform)
                            .is_none_or(|t| now - t >= crate::NAV_GAP_MS)
                });
                ready.then(|| p.caption_queue.pop_front()).flatten()
            };
            if let Some((mission_id, post, url)) = next_caption {
                let platform = platform_of_url(&url).to_string();
                {
                    let mut p = plan.borrow_mut();
                    p.busy.insert(platform.clone());
                    p.last_nav.insert(platform.clone(), host::now_ms());
                }
                let (this, plan) = (this.clone(), plan.clone());
                spawn_local(async move {
                    let outcome = fetch_captions(&this, &plan, &platform, &mission_id, &post).await;
                    let mut p = plan.borrow_mut();
                    p.busy.remove(&platform);
                    p.outcome.insert(mission_id, outcome);
                });
            }
            // One download at a time, a pause between two (D258).
            // A capture may record in the platform's tab: it waits for it.
            let next_download = {
                let mut p = plan.borrow_mut();
                let now = host::now_ms();
                let ready = !p.downloading
                    && now >= p.next_download_at
                    && p.downloads
                        .front()
                        .is_some_and(|(_, _, _, page)| !p.busy.contains(platform_of_url(page)));
                ready.then(|| p.downloads.pop_front()).flatten()
            };
            if let Some((mission_id, post, url, page)) = next_download {
                let platform = platform_of_url(&page).to_string();
                {
                    let mut p = plan.borrow_mut();
                    p.downloading = true;
                    p.busy.insert(platform.clone());
                }
                let (this, plan) = (this.clone(), plan.clone());
                spawn_local(async move {
                    let outcome =
                        capture_video(&this, &plan, &platform, &mission_id, &post, &url, &page)
                            .await;
                    let mut p = plan.borrow_mut();
                    p.busy.remove(&platform);
                    p.downloading = false;
                    p.next_download_at =
                        host::now_ms() + 5_000.0 + js_sys::Math::random() * 5_000.0;
                    p.outcome.insert(mission_id, outcome);
                });
            }
            let now = host::now_ms();
            if now - last_ping >= PING_MS {
                this.send(&AgentMsg::Ping);
                last_ping = now;
            }
            let due: Vec<AssignedMission> = {
                let p = plan.borrow();
                due_missions(&p.missions, &p.next, &p.busy, &p.last_nav, now)
                    .into_iter()
                    .cloned()
                    .collect()
            };
            for m in due {
                let platform = ba::platform_of(&m.target).to_string();
                {
                    let mut p = plan.borrow_mut();
                    p.busy.insert(platform.clone());
                    p.last_nav.insert(platform.clone(), now);
                    p.next.insert(
                        m.id.clone(),
                        next_run(now, m.interval_secs, js_sys::Math::random() * 2.0 - 1.0),
                    );
                }
                let (this, plan) = (this.clone(), plan.clone());
                spawn_local(async move {
                    let outcome = run_mission(&this, &plan, &platform, &m).await;
                    let mut p = plan.borrow_mut();
                    p.busy.remove(&platform);
                    p.outcome.insert(m.id.clone(), outcome);
                });
            }
            let p = plan.borrow();
            status.missions = p
                .missions
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "target": m.target,
                        "outcome": p.outcome.get(&m.id),
                    })
                })
                .collect();
            status.missing_origins = p.missing.clone();
            status.publish();
        }
        let socket = this.shared.borrow().socket;
        host::ws_close(socket);
    }
}

/// Missions held and their schedule.
#[derive(Default)]
struct Plan {
    missions: Vec<AssignedMission>,
    next: BTreeMap<String, f64>,
    busy: BTreeSet<String>,
    last_nav: BTreeMap<String, f64>,
    /// Platform → the agent's tab (D255).
    tabs: BTreeMap<String, f64>,
    /// Last outcome per mission (value or state code), for the options page.
    outcome: BTreeMap<String, String>,
    /// Origins a mission needs and the user has not allowed yet.
    missing: BTreeSet<String>,
    /// Video URL of the posts last reported as seen, until the server
    /// answers which to capture.
    videos: BTreeMap<String, String>,
    /// Page of every post last reported as seen (captions are read there).
    post_urls: BTreeMap<String, String>,
    /// Captions to read: (mission, post, url).
    caption_queue: std::collections::VecDeque<(String, String, String)>,
    /// Videos to capture: (mission, post, file or watch URL, post page).
    downloads: std::collections::VecDeque<(String, String, String, String)>,
    /// Caption track a platform listed with the post (TikTok): (lang, URL).
    tracks: BTreeMap<String, (String, String)>,
    downloading: bool,
    next_download_at: f64,
}

impl Plan {
    fn replace(&mut self, missions: Vec<AssignedMission>) {
        let ids: BTreeSet<&str> = missions.iter().map(|m| m.id.as_str()).collect();
        self.next.retain(|id, _| ids.contains(id.as_str()));
        self.outcome.retain(|id, _| ids.contains(id.as_str()));
        self.missions = missions;
    }
}

/// Runs one mission in its platform tab; returns the outcome shown in the
/// options page.
async fn run_mission(
    session: &Session,
    plan: &Rc<RefCell<Plan>>,
    platform: &str,
    m: &AssignedMission,
) -> String {
    let url = match &m.spec {
        MissionSpec::CollectValue(p) => p.url.clone(),
        MissionSpec::CollectPage(p) => p.url.clone(),
        MissionSpec::FollowAccount(p) => p.platform.list_urls(&p.handle)[0].clone(),
        MissionSpec::WatchLive(p) => p.url.clone(),
        MissionSpec::CrawlSite(p) => format!("{}/", p.url),
        MissionSpec::CaptureProof(p) => p.url.clone(),
    };
    // Sites the mission reads: its page, or the platform's sites.
    let needed: Vec<String> = match &m.spec {
        MissionSpec::FollowAccount(p) => p.platform.sites().iter().map(|s| s.to_string()).collect(),
        _ => vec![url.clone()],
    };
    let state = |state: MissionState, code: &str| AgentMsg::State {
        mission_id: m.id.clone(),
        state,
        code: (!code.is_empty()).then(|| code.to_string()),
    };
    let mut blocked = false;
    for site in &needed {
        let allowed = host::has_origin(site)
            .await
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut p = plan.borrow_mut();
        if allowed {
            p.missing.remove(&origin_of(site));
        } else {
            p.missing.insert(origin_of(site));
            blocked = true;
        }
    }
    if blocked {
        session.send(&state(
            MissionState::Blocked,
            err_codes::EXT_PERMISSION_MISSING,
        ));
        return err_codes::EXT_PERMISSION_MISSING.into();
    }
    let tab = plan.borrow().tabs.get(platform).copied().unwrap_or(-1.0);
    // Responses of the previous visit are stale.
    take_responses(tab);
    let tab = match host::agent_tab(tab, &url).await {
        Ok(id) => id.as_f64().unwrap_or(-1.0),
        Err(e) => return err_text(&e),
    };
    plan.borrow_mut().tabs.insert(platform.to_string(), tab);
    let p = match &m.spec {
        MissionSpec::CollectValue(p) => p,
        MissionSpec::CollectPage(_) => return capture_page(session, m, tab).await,
        MissionSpec::FollowAccount(f) => {
            return follow_account(session, plan, m, f, tab, platform).await
        }
        MissionSpec::WatchLive(l) => return watch_live(session, plan, m, l, platform).await,
        MissionSpec::CrawlSite(c) => return crawl_site(session, m, c, tab).await,
        MissionSpec::CaptureProof(_) => return capture_proof(session, m, tab).await,
    };
    #[derive(Deserialize)]
    struct Read {
        found: bool,
        value: String,
    }
    let read = host::read_value(tab, &p.selector, &p.attr)
        .await
        .ok()
        .and_then(|v| serde_json::from_str::<Read>(&text(&v)).ok());
    match read {
        Some(r) if r.found => {
            session.send(&AgentMsg::Report {
                mission_id: m.id.clone(),
                value: r.value.clone(),
            });
            r.value
        }
        _ => {
            session.send(&state(
                MissionState::Running,
                err_codes::EXT_SELECTOR_NOT_FOUND,
            ));
            err_codes::EXT_SELECTOR_NOT_FOUND.into()
        }
    }
}

/// `collect_page`: the tab's readable text, uploaded signed (D261, D262).
async fn capture_page(session: &Session, m: &AssignedMission, tab: f64) -> String {
    #[derive(Deserialize)]
    struct Page {
        url: String,
        title: String,
        text: String,
    }
    let Some(page) = host::read_page(tab)
        .await
        .ok()
        .and_then(|v| serde_json::from_str::<Page>(&text(&v)).ok())
    else {
        return "read-failed".into();
    };
    let snapshot = ba::PageSnapshot {
        mission_id: m.id.clone(),
        url: page.url,
        title: page.title,
        text: page.text,
    };
    let Ok(body) = serde_json::to_string(&snapshot) else {
        return "encode-failed".into();
    };
    let sha = sha256_hex(body.as_bytes());
    let creds = &session.creds;
    let ts = host::now_ms() as i64;
    let signature = match host::sign(&ba::upload_message(&creds.device_id, ts, &m.id, &sha)).await {
        Ok(sig) => text(&sig),
        Err(e) => return err_text(&e),
    };
    let headers = serde_json::json!({ "X-Pnex-Ts": ts.to_string(), "X-Pnex-Signature": signature });
    let reply = call_with(
        "POST",
        &format!("{}/api/v1/extension/uploads/page", creds.server),
        &creds.token,
        "",
        &body,
        &headers.to_string(),
    )
    .await;
    match reply {
        Ok(r) if r.status == 201 => serde_json::from_str::<ba::PageStored>(&r.body)
            .map(|s| format!("v{}", s.version))
            .unwrap_or_default(),
        Ok(r) => error_of(&r),
        Err(e) => e,
    }
}

/// Signs (upload message over the SHA-256 of the body) and POSTs a JSON
/// body of a mission.
async fn signed_json(
    session: &Session,
    route: &str,
    mission_id: &str,
    body: &str,
) -> Result<HttpReply, String> {
    let creds = &session.creds;
    let ts = host::now_ms() as i64;
    let sha = sha256_hex(body.as_bytes());
    let signature = host::sign(&ba::upload_message(&creds.device_id, ts, mission_id, &sha))
        .await
        .map_err(|e| err_text(&e))?;
    let headers =
        serde_json::json!({ "X-Pnex-Ts": ts.to_string(), "X-Pnex-Signature": text(&signature) });
    call_with(
        "POST",
        &format!("{}{route}", creds.server),
        &creds.token,
        "",
        body,
        &headers.to_string(),
    )
    .await
}

/// `capture_proof` (D308, D310): full-page screenshot, cleaned HTML and
/// readable text of the page, each uploaded signed; the server seals the
/// proof once the three are in.
async fn capture_proof(session: &Session, m: &AssignedMission, tab: f64) -> String {
    let shot = match host::capture_full_page(tab, f64::from(ba::PROOF_HEIGHT_MAX)).await {
        Ok(v) => v,
        // No capture right on this site (host permission): the task waits.
        Err(e) => {
            session.send(&AgentMsg::State {
                mission_id: m.id.clone(),
                state: MissionState::Blocked,
                code: Some(err_codes::EXT_PERMISSION_MISSING.into()),
            });
            return err_text(&e);
        }
    };
    let html = host::read_clean_html(tab).await;
    let page = host::read_page(tab).await;
    #[derive(Deserialize)]
    struct Page {
        url: String,
        title: String,
        text: String,
    }
    let Some(page) = page
        .ok()
        .and_then(|v| serde_json::from_str::<Page>(&text(&v)).ok())
    else {
        return "read-failed".into();
    };
    let snapshot = serde_json::to_string(&ba::PageSnapshot {
        mission_id: m.id.clone(),
        url: page.url,
        title: page.title,
        text: page.text,
    })
    .unwrap_or_default();
    let text_blob = host::text_blob(&snapshot).await;
    let parts = [
        (ba::ProofPart::Screenshot, Ok(shot), "image/png"),
        (ba::ProofPart::Html, html, "application/octet-stream"),
        (ba::ProofPart::Text, text_blob, "application/json"),
    ];
    let mut last = String::new();
    for (part, blob, content_type) in parts {
        let Some(blob) = blob
            .ok()
            .and_then(|v| serde_json::from_str::<Blob>(&text(&v)).ok())
        else {
            return format!("{}-failed", part.wire());
        };
        let ts = host::now_ms() as i64;
        let message =
            ba::proof_upload_message(&session.creds.device_id, ts, &m.id, part, &blob.sha256);
        let headers = serde_json::json!({
            "X-Pnex-Ts": ts.to_string(),
            "X-Pnex-Mission": m.id,
            "X-Pnex-Part": part.wire(),
        });
        last = match upload(
            session,
            "/api/v1/extension/uploads/proof",
            &blob,
            &message,
            headers,
            content_type,
        )
        .await
        {
            Ok(r) if r.status == 201 => serde_json::from_str::<ba::ProofPartStored>(&r.body)
                .map(|s| {
                    if s.sealed {
                        format!("proof {}", s.proof_id)
                    } else {
                        part.wire().into()
                    }
                })
                .unwrap_or_default(),
            Ok(r) => return error_of(&r),
            Err(e) => return e,
        };
    }
    last
}

/// Paths an agent crawl never opens (not pages).
const CRAWL_SKIPPED: [&str; 12] = [
    ".pdf", ".jpg", ".jpeg", ".png", ".gif", ".webp", ".svg", ".zip", ".mp4", ".mp3", ".xml",
    ".docx",
];

/// `crawl_site` run by an agent (D307): the same walk as the server's, in
/// the platform tab, at the D258 pace, never off the origin; each page is
/// reported signed, the end of the walk too.
async fn crawl_site(
    session: &Session,
    m: &AssignedMission,
    p: &ba::CrawlSiteParams,
    first_tab: f64,
) -> String {
    #[derive(Deserialize)]
    struct Page {
        url: String,
        title: String,
        text: String,
    }
    let origin = p.url.clone();
    let mut queue: std::collections::VecDeque<(String, u32)> = std::collections::VecDeque::new();
    let mut known: BTreeSet<String> = BTreeSet::new();
    let start = format!("{origin}/");
    known.insert(start.clone());
    queue.push_back((start, 0));
    let (mut tab, mut first, mut read, mut truncated) = (first_tab, true, Vec::new(), false);
    while let Some((url, depth)) = queue.pop_front() {
        if read.len() >= p.max_pages as usize {
            truncated = true;
            break;
        }
        if !first {
            host::sleep(ba::CRAWL_DELAY_MS as f64).await;
            tab = match host::agent_tab(tab, &url).await {
                Ok(id) => id.as_f64().unwrap_or(tab),
                Err(_) => continue,
            };
        }
        first = false;
        let Some(page) = host::read_page(tab)
            .await
            .ok()
            .and_then(|v| serde_json::from_str::<Page>(&text(&v)).ok())
        else {
            continue;
        };
        // A redirect off the site is not followed (D307).
        if origin_of(&page.url) != origin {
            continue;
        }
        let report = ba::SitePageReport {
            mission_id: m.id.clone(),
            url: page.url.clone(),
            title: page.title,
            text: page.text,
            depth,
        };
        let body = serde_json::to_string(&report).unwrap_or_default();
        match signed_json(session, "/api/v1/extension/uploads/site-page", &m.id, &body).await {
            Ok(r) if r.status == 201 => {
                read.push(page.url.split('#').next().unwrap_or_default().to_string())
            }
            Ok(r) => return error_of(&r),
            Err(e) => return e,
        }
        if depth >= p.max_depth {
            continue;
        }
        let links: Vec<String> = host::read_links(tab)
            .await
            .ok()
            .and_then(|v| serde_json::from_str(&text(&v)).ok())
            .unwrap_or_default();
        for link in links {
            let path = link.strip_prefix(&origin).map(|rest| {
                rest.split(['?', '#'])
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
            });
            let Some(path) = path.filter(|p| p.is_empty() || p.starts_with('/')) else {
                continue;
            };
            if p.exclude.iter().any(|x| path.starts_with(x.as_str()))
                || CRAWL_SKIPPED.iter().any(|e| path.ends_with(e))
            {
                continue;
            }
            if known.insert(link.clone()) {
                queue.push_back((link, depth + 1));
            }
        }
    }
    let done = ba::CrawlDone {
        mission_id: m.id.clone(),
        urls: read.clone(),
        complete: !truncated,
    };
    let body = serde_json::to_string(&done).unwrap_or_default();
    match signed_json(
        session,
        "/api/v1/extension/uploads/crawl-done",
        &m.id,
        &body,
    )
    .await
    {
        Ok(r) if r.status == 204 => format!("{} pages", read.len()),
        Ok(r) => error_of(&r),
        Err(e) => e,
    }
}

/// A list that loads nothing for this many scrolls in a row has ended.
const END_AFTER_IDLE_SCROLLS: u32 = 4;

thread_local! {
    /// Platform JSON relayed by the page hook, per tab (bounded).
    static RESPONSES: RefCell<BTreeMap<i64, Vec<(String, String)>>> = const { RefCell::new(BTreeMap::new()) };
}

/// Called by the service worker for each response the page hook relays.
#[wasm_bindgen]
pub fn on_response(tab: f64, url: String, body: String) {
    RESPONSES.with(|r| {
        let mut all = r.borrow_mut();
        let list = all.entry(tab as i64).or_default();
        // Drained after every scroll; the cap only guards a runaway page.
        if list.len() < 30 {
            list.push((url, body));
        }
    });
}

fn take_responses(tab: f64) -> Vec<(String, String)> {
    RESPONSES.with(|r| r.borrow_mut().remove(&(tab as i64)).unwrap_or_default())
}

/// Posts of the platform's JSON documents, keyed by id.
fn read_posts(
    platform: ba::SocialPlatform,
    handle: &str,
    docs: &[(String, String)],
    posts: &mut BTreeMap<String, ba::SeenPost>,
    tracks: &mut BTreeMap<String, (String, String)>,
) {
    use crate::adapters::{tiktok, x, youtube};
    for (url, body) in docs {
        let found = match platform {
            ba::SocialPlatform::TikTok if tiktok::is_list_url(url) => {
                tiktok::posts_of(body, handle)
                    .into_iter()
                    .map(|(post, track)| {
                        if let Some(track) = track {
                            tracks.insert(post.platform_id.clone(), track);
                        }
                        post
                    })
                    .collect()
            }
            ba::SocialPlatform::X if x::is_timeline_url(url) => x::posts_of(body, handle),
            ba::SocialPlatform::YouTube if url.is_empty() || youtube::is_list_url(url) => {
                youtube::posts_of(body, handle)
            }
            _ => continue,
        };
        for post in found {
            posts.entry(post.platform_id.clone()).or_insert(post);
        }
    }
}

/// `follow_account` (D283, D284): reads each list of the account like a
/// person, as deep as the server says (the history grows visit after
/// visit), reports the posts with how deep it went and whether the list
/// ended; keeps their video URLs for the capture.
async fn follow_account(
    session: &Session,
    plan: &Rc<RefCell<Plan>>,
    m: &AssignedMission,
    params: &ba::FollowAccountParams,
    first_tab: f64,
    platform: &str,
) -> String {
    let depth = m.scroll_depth.max(ba::RECENT_DEPTH);
    let mut posts: BTreeMap<String, ba::SeenPost> = BTreeMap::new();
    let mut tracks: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut end_reached = true;
    let mut deepest = 0;
    let mut tab = first_tab;
    for (i, list) in params.platform.list_urls(&params.handle).iter().enumerate() {
        if i > 0 {
            // Next list of the same account: same tab, human gap (D258).
            host::sleep(crate::NAV_GAP_MS).await;
            take_responses(tab);
            tab = match host::agent_tab(tab, list).await {
                Ok(id) => id.as_f64().unwrap_or(-1.0),
                Err(e) => return err_text(&e),
            };
            plan.borrow_mut().tabs.insert(platform.to_string(), tab);
        }
        // What the page loaded before any scroll (YouTube keeps it in the page).
        if let Ok(initial) = host::read_initial_data(tab).await {
            let initial = text(&initial);
            if initial != "null" && !initial.is_empty() {
                read_posts(
                    params.platform,
                    &params.handle,
                    &[(String::new(), initial)],
                    &mut posts,
                    &mut tracks,
                );
            }
        }
        read_posts(
            params.platform,
            &params.handle,
            &take_responses(tab),
            &mut posts,
            &mut tracks,
        );
        // D269: some platforms pause a hidden tab; never read one blind.
        if params.platform.needs_foreground() {
            let _ = host::focus_tab(tab).await;
            if !visible(tab).await {
                session.send(&AgentMsg::State {
                    mission_id: m.id.clone(),
                    state: MissionState::Blocked,
                    code: Some(err_codes::EXT_TAB_HIDDEN.into()),
                });
                return err_codes::EXT_TAB_HIDDEN.into();
            }
        }
        let mut idle = 0;
        let mut scrolls = 0;
        while scrolls < depth {
            host::sleep(3_000.0 + js_sys::Math::random() * 3_000.0).await;
            if host::scroll(tab).await.is_err() {
                break;
            }
            scrolls += 1;
            host::sleep(1_500.0).await;
            let docs = take_responses(tab);
            if docs.is_empty() {
                idle += 1;
                if idle >= END_AFTER_IDLE_SCROLLS {
                    break;
                }
            } else {
                idle = 0;
                read_posts(
                    params.platform,
                    &params.handle,
                    &docs,
                    &mut posts,
                    &mut tracks,
                );
            }
        }
        deepest = deepest.max(scrolls);
        // A list ends when it stopped loading before the depth asked.
        end_reached &= scrolls < depth;
    }
    if posts.is_empty() {
        // Nothing read: a sign-in or consent wall, or the page changed.
        let page = host::read_page(tab)
            .await
            .ok()
            .map(|v| text(&v))
            .unwrap_or_default();
        let code =
            if page.contains("/login") || page.contains("/i/flow/") || page.contains("consent.") {
                err_codes::EXT_LOGIN_REQUIRED
            } else {
                err_codes::EXT_SELECTOR_NOT_FOUND
            };
        session.send(&AgentMsg::State {
            mission_id: m.id.clone(),
            state: MissionState::Blocked,
            code: Some(code.into()),
        });
        return code.into();
    }
    let posts: Vec<ba::SeenPost> = posts.into_values().collect();
    {
        let mut p = plan.borrow_mut();
        for post in &posts {
            if let Some(url) = &post.video_url {
                p.videos.insert(post.platform_id.clone(), url.clone());
                p.post_urls
                    .insert(post.platform_id.clone(), post.url.clone());
            }
        }
        p.tracks.extend(tracks);
    }
    let count = posts.len();
    // Batches within the server's bound; only the last one carries the end.
    let batches: Vec<Vec<ba::SeenPost>> = posts.chunks(ba::SEEN_MAX).map(<[_]>::to_vec).collect();
    let last = batches.len().saturating_sub(1);
    for (i, batch) in batches.into_iter().enumerate() {
        session.send(&AgentMsg::Seen {
            mission_id: m.id.clone(),
            posts: batch,
            depth: deepest,
            end_reached: end_reached && i == last,
        });
    }
    format!("{count} posts")
}

/// Platform key of a post page (the tab it is read in).
fn platform_of_url(url: &str) -> &'static str {
    if url.starts_with("https://www.youtube.com/") {
        "youtube"
    } else if url.starts_with("https://www.tiktok.com/") {
        "tiktok"
    } else {
        "x"
    }
}

/// Reads a post's platform captions on its page and uploads them signed
/// (D281). YouTube only: X posts rarely carry captions.
async fn fetch_captions(
    session: &Session,
    plan: &Rc<RefCell<Plan>>,
    platform: &str,
    mission_id: &str,
    post: &str,
) -> String {
    use crate::adapters::{tiktok, youtube};
    let (lang, cues) = match platform {
        // TikTok listed the track with the post: no page to open.
        "tiktok" => {
            let Some((lang, url)) = plan.borrow().tracks.get(post).cloned() else {
                return "no-captions".into();
            };
            match host::fetch_text(&url).await {
                Ok(body) => (lang, tiktok::vtt_cues(&text(&body))),
                Err(e) => return err_text(&e),
            }
        }
        "youtube" => {
            let tab = plan.borrow().tabs.get(platform).copied().unwrap_or(-1.0);
            let watch = format!("https://www.youtube.com/watch?v={post}");
            let tab = match host::agent_tab(tab, &watch).await {
                Ok(id) => id.as_f64().unwrap_or(-1.0),
                Err(e) => return err_text(&e),
            };
            plan.borrow_mut().tabs.insert(platform.to_string(), tab);
            take_responses(tab);
            let player = host::read_player_response(tab)
                .await
                .map(|v| text(&v))
                .unwrap_or_default();
            let Some((lang, url)) = youtube::caption_track(&player) else {
                return "no-captions".into();
            };
            match host::fetch_text(&url).await {
                Ok(body) => (lang, youtube::cues_of(&text(&body))),
                Err(e) => return err_text(&e),
            }
        }
        // X posts rarely carry captions.
        _ => return "no-captions".into(),
    };
    if cues.is_empty() {
        return "no-captions".into();
    }
    let upload = ba::CaptionsUpload {
        mission_id: mission_id.into(),
        platform_id: post.into(),
        lang,
        cues,
    };
    let Ok(body) = serde_json::to_string(&upload) else {
        return "encode-failed".into();
    };
    let sha = sha256_hex(body.as_bytes());
    let creds = &session.creds;
    let ts = host::now_ms() as i64;
    let message = ba::captions_upload_message(&creds.device_id, ts, mission_id, post, &sha);
    let signature = match host::sign(&message).await {
        Ok(sig) => text(&sig),
        Err(e) => return err_text(&e),
    };
    let headers = serde_json::json!({ "X-Pnex-Ts": ts.to_string(), "X-Pnex-Signature": signature });
    match call_with(
        "POST",
        &format!("{}/api/v1/extension/uploads/captions", creds.server),
        &creds.token,
        "",
        &body,
        &headers.to_string(),
    )
    .await
    {
        Ok(r) if r.status == 201 => format!("captions {post}"),
        Ok(r) => error_of(&r),
        Err(e) => e,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The tab is shown on screen (window not minimized nor covered, screen
/// not locked).
async fn visible(tab: f64) -> bool {
    host::is_tab_visible(tab)
        .await
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// A file ready to upload (kept on the JS side, see host.js).
#[derive(Deserialize)]
struct Blob {
    id: f64,
    sha256: String,
}

/// Bitrate of a browser recording: ~5 MB a minute, 40 minutes under the
/// upload cap.
const RECORD_BITRATE: f64 = 600_000.0;

/// Records what `page` plays in the platform's tab, in real time (D260).
async fn record(
    plan: &Rc<RefCell<Plan>>,
    platform: &str,
    page: &str,
    max_ms: f64,
    live: bool,
    navigate: bool,
    from_ms: f64,
) -> Result<(Blob, String), String> {
    let mut tab = plan.borrow().tabs.get(platform).copied().unwrap_or(-1.0);
    if navigate || tab < 0.0 {
        tab = host::agent_tab(tab, page)
            .await
            .map_err(|e| err_text(&e))?
            .as_f64()
            .unwrap_or(-1.0);
        plan.borrow_mut().tabs.insert(platform.to_string(), tab);
    }
    if ba::is_exclusive(platform) {
        let _ = host::focus_tab(tab).await;
        if !visible(tab).await {
            return Err(err_codes::EXT_TAB_HIDDEN.into());
        }
    }
    let id = host::random_b64(12);
    host::start_recording(
        tab,
        &id,
        max_ms,
        RECORD_BITRATE,
        live,
        ba::VIDEO_MAX as f64,
        from_ms,
    )
    .await
    .map_err(|e| err_text(&e))?;
    #[derive(Deserialize)]
    struct State {
        done: bool,
        reason: Option<String>,
    }
    let started = host::now_ms();
    let reason = loop {
        host::sleep(2_000.0).await;
        let state: State = serde_json::from_str(&host::recording_state(&id)).unwrap_or(State {
            done: true,
            reason: Some("lost".into()),
        });
        if state.done {
            break state.reason.unwrap_or_else(|| "ended".into());
        }
        if host::now_ms() - started > max_ms + 120_000.0 {
            break "timeout".into();
        }
    };
    if matches!(reason.as_str(), "no-video" | "no-capture" | "lost")
        || reason == err_codes::EXT_DRM_PROTECTED
    {
        return Err(reason);
    }
    let blob = host::recording_to_blob(&id)
        .await
        .map_err(|e| err_text(&e))?;
    let blob: Blob = serde_json::from_str(&text(&blob)).map_err(|e| e.to_string())?;
    Ok((blob, reason))
}

/// Signs and uploads a file to an agent upload route.
async fn upload(
    session: &Session,
    route: &str,
    blob: &Blob,
    message: &str,
    mut headers: serde_json::Value,
    content_type: &str,
) -> Result<HttpReply, String> {
    let creds = &session.creds;
    let signature = host::sign(message).await.map_err(|e| err_text(&e))?;
    headers["X-Pnex-Signature"] = text(&signature).into();
    let reply = host::upload_blob(
        blob.id,
        &format!("{}{route}", creds.server),
        &creds.token,
        &headers.to_string(),
        content_type,
    )
    .await
    .map_err(|e| err_text(&e))?;
    serde_json::from_str::<HttpReply>(&text(&reply)).map_err(|e| e.to_string())
}

/// Captures one video: its file when the platform exposes it, else a
/// recording of its page (D260); uploaded signed (D262).
async fn capture_video(
    session: &Session,
    plan: &Rc<RefCell<Plan>>,
    platform: &str,
    mission_id: &str,
    post: &str,
    url: &str,
    page: &str,
) -> String {
    let watch_page = url == page || url.starts_with("https://www.youtube.com/");
    let fetched = if watch_page {
        None
    } else {
        host::fetch_blob(url, ba::VIDEO_MAX as f64)
            .await
            .ok()
            .and_then(|v| serde_json::from_str::<Blob>(&text(&v)).ok())
    };
    if let Some(blob) = fetched {
        return upload_video(session, mission_id, post, 1, &blob, "video/mp4").await;
    }
    // No file: recorded in real time, in parts like a live (O92), each part
    // starting where the previous one stopped.
    let part_ms = ba::LIVE_PART_MAX_MS as f64;
    for part in 1..=ba::VIDEO_PARTS_MAX {
        let from_ms = f64::from(part - 1) * part_ms;
        let (blob, reason) =
            match record(plan, platform, page, part_ms, false, part == 1, from_ms).await {
                Ok(done) => done,
                Err(reason) => return reason,
            };
        let sent = upload_video(session, mission_id, post, part, &blob, "video/webm").await;
        if !sent.starts_with("video ") || (reason != "max-duration" && reason != "max-size") {
            return sent;
        }
    }
    format!("video {post}: {} parts, cut", ba::VIDEO_PARTS_MAX)
}

/// Signs and uploads one file (or part) of a post's video.
async fn upload_video(
    session: &Session,
    mission_id: &str,
    post: &str,
    part: u32,
    blob: &Blob,
    content_type: &str,
) -> String {
    let creds = &session.creds;
    let ts = host::now_ms() as i64;
    let message =
        ba::video_upload_message(&creds.device_id, ts, mission_id, post, part, &blob.sha256);
    let headers = serde_json::json!({
        "X-Pnex-Ts": ts.to_string(),
        "X-Pnex-Mission": mission_id,
        "X-Pnex-Post": post,
        "X-Pnex-Part": part.to_string(),
    });
    match upload(
        session,
        "/api/v1/extension/uploads/video",
        blob,
        &message,
        headers,
        content_type,
    )
    .await
    {
        Ok(r) if r.status == 201 => format!("video {post}"),
        Ok(r) => error_of(&r),
        Err(e) => e,
    }
}

/// `watch_live` (D260): records the live part after part (each at most
/// [`ba::LIVE_PART_MAX_MS`]) until it ends; each part is a media.
async fn watch_live(
    session: &Session,
    plan: &Rc<RefCell<Plan>>,
    m: &AssignedMission,
    params: &ba::WatchLiveParams,
    platform: &str,
) -> String {
    let mut part: u32 = 0;
    loop {
        part += 1;
        let started = String::from(js_sys::Date::new_0().to_iso_string());
        // The first part opens the page; the next ones keep it playing.
        let (blob, reason) = match record(
            plan,
            platform,
            &params.url,
            ba::LIVE_PART_MAX_MS as f64,
            true,
            part == 1,
            0.0,
        )
        .await
        {
            Ok(done) => done,
            Err(reason) => {
                if part == 1 {
                    session.send(&AgentMsg::State {
                        mission_id: m.id.clone(),
                        state: MissionState::Blocked,
                        code: (reason == err_codes::EXT_DRM_PROTECTED).then(|| reason.clone()),
                    });
                }
                return reason;
            }
        };
        let creds = &session.creds;
        let ts = host::now_ms() as i64;
        let message = ba::live_upload_message(&creds.device_id, ts, &m.id, part, &blob.sha256);
        let headers = serde_json::json!({
            "X-Pnex-Ts": ts.to_string(),
            "X-Pnex-Mission": m.id,
            "X-Pnex-Part": part.to_string(),
            "X-Pnex-Started": started,
        });
        match upload(
            session,
            "/api/v1/extension/uploads/live",
            &blob,
            &message,
            headers,
            "video/webm",
        )
        .await
        {
            Ok(r) if r.status == 201 => {}
            Ok(r) => return error_of(&r),
            Err(e) => return e,
        }
        // A part cut at its maximum means the live goes on.
        if reason != "max-duration" && reason != "max-size" {
            return format!("live: {part} parts");
        }
    }
}

/// `https://host` of a page URL.
fn origin_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    format!("{scheme}://{host}")
}
