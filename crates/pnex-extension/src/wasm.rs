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
                    } => {
                        let mut p = plan.borrow_mut();
                        for id in platform_ids {
                            if let Some(url) = p.videos.remove(&id) {
                                p.downloads.push_back((mission_id.clone(), id, url));
                            }
                        }
                    }
                    ServerMsg::Pong => {}
                }
            }
            // One download at a time, a pause between two (D258).
            let next_download = {
                let mut p = plan.borrow_mut();
                let ready = !p.downloading && host::now_ms() >= p.next_download_at;
                ready.then(|| p.downloads.pop_front()).flatten()
            };
            if let Some((mission_id, post, url)) = next_download {
                plan.borrow_mut().downloading = true;
                let (this, plan) = (this.clone(), plan.clone());
                spawn_local(async move {
                    let outcome = capture_video(&this, &mission_id, &post, &url).await;
                    let mut p = plan.borrow_mut();
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
    /// Videos to capture: (mission, post, url).
    downloads: std::collections::VecDeque<(String, String, String)>,
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
        MissionSpec::FollowAccount(p) => p.platform.profile_url(&p.handle),
    };
    // Sites the mission reads: its page, plus the platform's video host.
    let mut needed = vec![url.clone()];
    if let MissionSpec::FollowAccount(_) = &m.spec {
        needed.push(X_VIDEO_HOST.into());
    }
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
        MissionSpec::FollowAccount(f) => return follow_account(session, plan, m, f, tab).await,
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
    use sha2::Digest as _;
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
    let sha: String = sha2::Sha256::digest(body.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
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

/// Host of X's video files (its own permission, asked with x.com).
const X_VIDEO_HOST: &str = "https://video.twimg.com/";
/// Scrolls of a profile per visit (3–6 s apart).
const PROFILE_SCROLLS: u32 = 3;

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
        if list.len() < 30 {
            list.push((url, body));
        }
    });
}

fn take_responses(tab: f64) -> Vec<(String, String)> {
    RESPONSES.with(|r| r.borrow_mut().remove(&(tab as i64)).unwrap_or_default())
}

/// `follow_account`: reads the profile like a person (a few slow scrolls),
/// reports the account's posts, keeps their video URLs for the capture.
async fn follow_account(
    session: &Session,
    plan: &Rc<RefCell<Plan>>,
    m: &AssignedMission,
    params: &ba::FollowAccountParams,
    tab: f64,
) -> String {
    for _ in 0..PROFILE_SCROLLS {
        host::sleep(3_000.0 + js_sys::Math::random() * 3_000.0).await;
        if host::scroll(tab).await.is_err() {
            break;
        }
    }
    host::sleep(3_000.0).await;
    let mut posts: BTreeMap<String, ba::SeenPost> = BTreeMap::new();
    for (url, body) in take_responses(tab) {
        if crate::adapters::x::is_timeline_url(&url) {
            for post in crate::adapters::x::posts_of(&body, &params.handle) {
                posts.entry(post.platform_id.clone()).or_insert(post);
            }
        }
    }
    if posts.is_empty() {
        // Nothing relayed: a sign-in wall, or the account's page changed.
        let page = host::read_page(tab)
            .await
            .ok()
            .map(|v| text(&v))
            .unwrap_or_default();
        let code = if page.contains("/login") || page.contains("/i/flow/") {
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
    let posts: Vec<ba::SeenPost> = posts.into_values().take(ba::SEEN_MAX).collect();
    {
        let mut p = plan.borrow_mut();
        for post in &posts {
            if let Some(url) = &post.video_url {
                p.videos.insert(post.platform_id.clone(), url.clone());
            }
        }
    }
    let count = posts.len();
    session.send(&AgentMsg::Seen {
        mission_id: m.id.clone(),
        posts,
    });
    format!("{count} posts")
}

/// Downloads one video and uploads it signed (D260, D262).
async fn capture_video(session: &Session, mission_id: &str, post: &str, url: &str) -> String {
    #[derive(Deserialize)]
    struct Blob {
        id: f64,
        sha256: String,
    }
    let blob = match host::fetch_blob(url, ba::VIDEO_MAX as f64).await {
        Ok(v) => match serde_json::from_str::<Blob>(&text(&v)) {
            Ok(b) => b,
            Err(e) => return e.to_string(),
        },
        Err(e) => return err_text(&e),
    };
    let creds = &session.creds;
    let ts = host::now_ms() as i64;
    let message = ba::video_upload_message(&creds.device_id, ts, mission_id, post, &blob.sha256);
    let signature = match host::sign(&message).await {
        Ok(sig) => text(&sig),
        Err(e) => return err_text(&e),
    };
    let headers = serde_json::json!({
        "X-Pnex-Ts": ts.to_string(),
        "X-Pnex-Signature": signature,
        "X-Pnex-Mission": mission_id,
        "X-Pnex-Post": post,
    });
    let reply = host::upload_blob(
        blob.id,
        &format!("{}/api/v1/extension/uploads/video", creds.server),
        &creds.token,
        &headers.to_string(),
    )
    .await;
    match reply.map(|v| serde_json::from_str::<HttpReply>(&text(&v))) {
        Ok(Ok(r)) if r.status == 201 => format!("video {post}"),
        Ok(Ok(r)) => error_of(&r),
        Ok(Err(e)) => e.to_string(),
        Err(e) => err_text(&e),
    }
}

/// `https://host` of a page URL.
fn origin_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    format!("{scheme}://{host}")
}
