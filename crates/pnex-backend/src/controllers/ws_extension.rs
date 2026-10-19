//! `GET /ws/extension?ticket=` — link of a browser agent
//! (extension-collector.md D262).
//!
//! - admission: one-time ticket from `POST /api/v1/extension/ws-ticket`
//!   (4001, obtained over HTTPS with the agent's token), then the Noise
//!   NNpsk0 handshake with the agent's key (4011). No device-edge check
//!   (4013): a browser reaches the app address, never the mTLS device edge
//!   that stamps `X-Pnex-Edge` (SEC-50);
//! - presence: the device liveness lease (D108), one session per agent
//!   (4003);
//! - every [`REFRESH`]: re-reads the missions, claims or renews the lease
//!   of their targets (D257) and sends the held set when it changed;
//! - agent → server: `report` (value of a `collect_value`), `seen` (posts
//!   of a `follow_account`, answered by `capture`), `state`, `ping`; only
//!   for missions this session holds.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use loco_rs::prelude::*;
use pnex_core::browser_agent::{AgentMsg, AssignedMission, MissionSpec, MissionState, ServerMsg};
use pnex_core::err_codes;
use uuid::Uuid;

use super::device_link::{close_socket, reject, FrameCodec};
use crate::services::device_liveness;
use crate::services::extension::{self, AgentAuth};
use crate::services::settings::IngestSettings;
use crate::services::telemetry::{self, TelemetryPoint};

/// Missions re-read, leases renewed (well under `LEASE_TTL_MS`).
const REFRESH: Duration = Duration::from_secs(15);
/// A silent agent is dropped (it pings every 30 s).
const WATCHDOG: Duration = Duration::from_secs(90);
/// Largest sealed frame an agent may send (a `seen` batch of posts).
const MAX_WIRE: usize = 1024 * 1024;
/// Most `seen` batches a link records per minute; more are dropped. An
/// agent scrolls a platform every 4.5 s at best, three platforms at once.
const SEEN_PER_MINUTE: u32 = 60;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/ws")
        .add("/extension", get(ws_extension))
}

#[derive(serde::Deserialize)]
struct TicketQuery {
    ticket: Option<String>,
}

async fn ws_extension(
    State(ctx): State<AppContext>,
    Query(q): Query<TicketQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let settings = IngestSettings::from_config(&ctx.config);
    let Some(id) = extension::redeem_ticket(&ctx.config, q.ticket.as_deref().unwrap_or("")).await
    else {
        return reject(ws, 4001, "Authentication failed");
    };
    let Ok(Some(agent)) = extension::agent_by_id(&ctx.db, id).await else {
        return reject(ws, 4001, "Authentication failed");
    };
    ws.max_message_size(MAX_WIRE)
        .on_upgrade(move |mut socket| async move {
            let Some(codec) =
                FrameCodec::accept(&mut socket, &agent.encryption_key, &agent.device_id, false)
                    .await
            else {
                return;
            };
            let session = Uuid::new_v4().simple().to_string();
            match device_liveness::claim(
                agent.device_registry_id,
                &session,
                settings.silence_ttl_secs,
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    return close_socket(&mut socket, 4003, "Agent already connected").await
                }
                Err(_) => return close_socket(&mut socket, 1013, "Try again later").await,
            }
            let mut link = Link {
                ctx,
                agent,
                session,
                silence_ttl_secs: settings.silence_ttl_secs,
                held: BTreeMap::new(),
                announced: false,
                seen_window: (tokio::time::Instant::now(), 0),
            };
            link.run(socket, codec).await;
            link.close().await;
        })
        .into_response()
}

struct Link {
    ctx: AppContext,
    agent: AgentAuth,
    session: String,
    silence_ttl_secs: i64,
    /// Missions held by this session, by id.
    held: BTreeMap<String, AssignedMission>,
    /// The held set was sent at least once.
    announced: bool,
    /// Start of the current minute and `seen` batches recorded in it.
    seen_window: (tokio::time::Instant, u32),
}

impl Link {
    async fn run(&mut self, mut socket: WebSocket, mut codec: FrameCodec) {
        let mut tick = tokio::time::interval(REFRESH);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tracing::info!(agent = %self.agent.device_id, "browser agent link up");
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if !extension::still_paired(&self.ctx.db, &self.agent).await {
                        close_socket(&mut socket, 4005, "Agent unpaired").await;
                        return;
                    }
                    if let Ok(false) = device_liveness::touch_owned(
                        self.agent.device_registry_id, &self.session, self.silence_ttl_secs,
                    ).await {
                        close_socket(&mut socket, 4003, "Superseded").await;
                        return;
                    }
                    if self.refresh().await && !send(&mut socket, &mut codec, &self.desired()).await {
                        return;
                    }
                }
                incoming = tokio::time::timeout(WATCHDOG, socket.recv()) => {
                    let text = match incoming {
                        Ok(Some(Ok(Message::Text(t)))) => t,
                        Ok(Some(Ok(Message::Close(_)) | Err(_))) | Ok(None) | Err(_) => return,
                        Ok(Some(Ok(_))) => continue,
                    };
                    // A refused frame desynchronizes the Noise counters: close.
                    let Some(plain) = codec.open(text.as_str()) else {
                        tracing::warn!(agent = %self.agent.device_id, "unauthenticated frame, closing");
                        return;
                    };
                    let Ok(msg) = serde_json::from_str::<AgentMsg>(&plain) else {
                        continue;
                    };
                    if let Some(answer) = self.handle(msg).await {
                        if !send(&mut socket, &mut codec, &answer).await {
                            return;
                        }
                    }
                }
            }
        }
    }

    fn desired(&self) -> ServerMsg {
        ServerMsg::Missions {
            missions: self.held.values().cloned().collect(),
        }
    }

    /// Re-reads the candidates and the leases; `true` when the held set
    /// changed (the first call always sends).
    async fn refresh(&mut self) -> bool {
        let candidates = match extension::candidates(&self.ctx.db, &self.agent).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(agent = %self.agent.device_id, error = %e, "missions unavailable");
                return false;
            }
        };
        let mut granted: BTreeMap<&str, bool> = BTreeMap::new();
        let mut next = BTreeMap::new();
        for m in &candidates {
            let ok = match granted.get(m.target.as_str()) {
                Some(ok) => *ok,
                None => {
                    let ok = extension::claim_lease(
                        &self.ctx.config,
                        self.agent.org_id,
                        &m.target,
                        &self.session,
                    )
                    .await;
                    granted.insert(&m.target, ok);
                    ok
                }
            };
            if ok {
                next.insert(m.id.clone(), m.clone());
            }
        }
        let kept: BTreeSet<&str> = next.values().map(|m| m.target.as_str()).collect();
        for gone in self
            .held
            .values()
            .filter(|m| !kept.contains(m.target.as_str()))
        {
            extension::release_lease(
                &self.ctx.config,
                self.agent.org_id,
                &gone.target,
                &self.session,
            )
            .await;
        }
        let changed = next != self.held || !self.announced;
        self.held = next;
        self.announced = true;
        changed
    }

    async fn handle(&mut self, msg: AgentMsg) -> Option<ServerMsg> {
        match msg {
            AgentMsg::Ping => Some(ServerMsg::Pong),
            AgentMsg::Seen {
                mission_id,
                posts,
                depth,
                end_reached,
            } => {
                self.held_id(&mission_id)?;
                if !self.seen_allowed() {
                    tracing::warn!(agent = %self.agent.device_id, "seen batch over the rate, dropped");
                    return None;
                }
                match extension::record_seen(
                    &self.ctx.db,
                    &self.agent,
                    &mission_id,
                    &posts,
                    depth,
                    end_reached,
                )
                .await
                {
                    Ok((platform_ids, captions)) => Some(ServerMsg::Capture {
                        mission_id,
                        platform_ids,
                        captions,
                    }),
                    Err(e) => {
                        tracing::warn!(agent = %self.agent.device_id, error = %e, "seen posts not recorded");
                        None
                    }
                }
            }
            AgentMsg::State {
                mission_id,
                state,
                code,
            } => {
                let id = self.held_id(&mission_id)?;
                self.save_state(id, state, code.as_deref()).await;
                None
            }
            AgentMsg::Report { mission_id, value } => {
                let id = self.held_id(&mission_id)?;
                // Only a `collect_value` reports a value; pages are uploaded.
                let MissionSpec::CollectValue(params) = &self.held.get(&mission_id)?.spec else {
                    return None;
                };
                let number = value
                    .trim()
                    .chars()
                    .take(pnex_core::browser_agent::VALUE_MAX)
                    .collect::<String>()
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite());
                match number {
                    Some(n) => {
                        telemetry::sink().send(TelemetryPoint {
                            org_id: self.agent.org_id,
                            device_registry_id: self.agent.device_registry_id,
                            device_id: self.agent.device_id.clone(),
                            pred_dev: pnex_core::BROWSER_AGENT_PREDEF.into(),
                            metric_name: params.metric.clone(),
                            value: n.to_string(),
                            timestamp: chrono::Utc::now(),
                            ts_source: "server",
                            source_type: "browser_agent",
                            record: true,
                        });
                        self.save_state(id, MissionState::Running, None).await;
                    }
                    None => {
                        self.save_state(
                            id,
                            MissionState::Running,
                            Some(err_codes::EXT_VALUE_NOT_NUMERIC),
                        )
                        .await
                    }
                }
                None
            }
        }
    }

    fn seen_allowed(&mut self) -> bool {
        let (start, count) = &mut self.seen_window;
        if start.elapsed() >= Duration::from_secs(60) {
            *start = tokio::time::Instant::now();
            *count = 0;
        }
        *count += 1;
        *count <= SEEN_PER_MINUTE
    }

    /// Mission id, only when this session holds it.
    fn held_id(&self, mission_id: &str) -> Option<Uuid> {
        self.held
            .contains_key(mission_id)
            .then(|| Uuid::parse_str(mission_id).ok())
            .flatten()
    }

    async fn save_state(&self, id: Uuid, state: MissionState, code: Option<&str>) {
        if let Err(e) = extension::report_state(&self.ctx.db, &self.agent, id, state, code).await {
            tracing::warn!(agent = %self.agent.device_id, error = %e, "mission state not saved");
        }
    }

    async fn close(&self) {
        let targets: BTreeSet<&str> = self.held.values().map(|m| m.target.as_str()).collect();
        for target in targets {
            extension::release_lease(&self.ctx.config, self.agent.org_id, target, &self.session)
                .await;
        }
        let _ =
            device_liveness::release(&self.ctx.db, self.agent.device_registry_id, &self.session)
                .await;
        tracing::info!(agent = %self.agent.device_id, "browser agent link down");
    }
}

async fn send(socket: &mut WebSocket, codec: &mut FrameCodec, msg: &ServerMsg) -> bool {
    let Ok(plain) = serde_json::to_string(msg) else {
        return false;
    };
    socket
        .send(Message::Text(codec.seal(&plain).into()))
        .await
        .is_ok()
}
