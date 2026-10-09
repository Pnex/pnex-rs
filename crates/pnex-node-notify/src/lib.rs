//! Nœud custom EdgeLinkd `pnex-notify` (D49–D54) — notification
//! multi-canaux rendue depuis un template minijinja de l'org.
//!
//! Doctrine D50 (snapshot au deploy) : ce nœud porte dans l'artefact un
//! **snapshot** résolu par le backend (`pnex_notify_channels` enabled-only +
//! `pnex_notify_template`) — il ne fait **aucune** résolution ni aller-
//! retour backend au send (le backend ne lie jamais le moteur). Éditer un
//! canal/template ne mute pas les flows déjà déployés : re-déployer.
//!
//! Sémantique D53 : **lenient par défaut** — un échec d'envoi ne fait pas
//! tomber le flow (warn + passthrough) ; `strict: true` → le message est
//! rejeté (erreur routée vers `flow.handle_error`, flow_error isolé,
//! jamais de crash-loop). Le msg est **toujours** forwardé en sortie.
//!
//! Named-anchor mode (non-empty template vars stamped in the snapshot):
//! "complete set then clear" accumulation — each message fills the vars
//! (topic = name via the deploy taggers, else the keys of an object
//! payload), **last value wins** so a waiting set always carries the most
//! recent data; render + send only happen once the set is complete, then
//! reset. Snapshot without vars ⇒ immediate mode: render + send on every
//! message; with the trigger wired (always, see below) that template has no
//! data anchor, so the trigger's rising edge (false → true) renders + sends
//! it once; it fires again only after a `false` (O23). Anti-spam (fixed window "max N / D s"): the first send opens the
//! window, the surplus is a warn — in anchor mode the send is deferred (set
//! kept and kept up to date), never lost.
//!
//! Boolean trigger gate (deploy-derived `pnex_notify_trigger`): when a wire
//! is annotated on the permanent `trigger` input row, the projection
//! inserts a tagger stamping `topic = "trigger"` and sets the flag. A
//! message on that topic is a gate update — never template data — and is
//! consumed (no passthrough): the payload arms/disarms the node (Node-RED
//! style truthiness: booleans, non-zero numbers, `"true"`/`"false"`/`"1"`/
//! `"0"` strings; anything else disarms); data sends only flow while armed,
//! and an armed `true` commits a pending complete set (accumulation mode).
//! The commit is not immediate: it fires after a short grace window
//! (`COMMIT_GRACE`) so data fanned out from the same source tick as the
//! trigger — which may reach this node after the trigger — is folded into
//! the set before rendering (otherwise the previous tick's values are sent).
//! The gate is MANDATORY: an artifact without the flag (unwired trigger) is
//! refused at build — the old always-send semantics spammed on fast sources.
//!
//! Secrets : les canaux `websocket` reçoivent leurs coordonnées
//! (`PNEX_NOTIFY_DELIVER_URL`/`_TOKEN`) de l'env du runtime — absentes au
//! build ⇒ `BadFlowsJson` explicite.
//!
//! Vault (secrets.md D115, lot S4): the snapshot only carries
//! `{"secret_id"}` references. The node fetches their values from
//! `PNEX_FLOW_SECRET_URL` on its first send (the backend only serves a
//! secret once the flow is marked deployed, which happens right after the
//! runtime acknowledged the artifact) and keeps them in memory until the
//! next deploy (D118). Values are never logged.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

/// Point d'ancrage référencé par le binaire `pnex-flow-runtime` : garantit que
/// l'édition de liens conserve les soumissions `inventory` de ce crate.
pub fn registered() {}

/// Canal résolu du snapshot — l'artefact ne transporte jamais les canaux
/// désactivés (filtrés au deploy, cf. `resolve_notify_snapshots`).
#[derive(Debug, Deserialize)]
struct ChannelSnapshot {
    id: String,
    kind: String,
    #[serde(default)]
    config: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct TemplateSnapshot {
    #[serde(default)]
    subject: Option<String>,
    body: String,
    /// Vars de template stampées au deploy depuis la colonne mergée
    /// (« détectées + déclarées ») — non vide ⇒ mode accumulation « set
    /// complet puis clear » : chaque message remplit les vars (topic = nom
    /// via les taggers, sinon clés du payload objet), le rendu + envoi
    /// n'ont lieu qu'au set complet, puis reset. Vide ⇒ mode immédiat
    /// (rendu + envoi à chaque message, vieux flows inchangés).
    #[serde(default)]
    vars: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PnexNotifyNodeConfig {
    /// Estampilles de traçabilité du deploy.
    #[serde(default)]
    pnex_flow_id: i64,
    #[serde(default)]
    pnex_org_id: i64,
    /// Template of the node (journal context only).
    #[serde(default)]
    template_id: Option<String>,
    /// Graph node id stamped at deploy (journal + message meta).
    #[serde(default)]
    pnex_node_id: String,
    /// Un échec d'envoi rejette le message (strict) ou passe sous silence
    /// (lenient, défaut).
    #[serde(default)]
    strict: bool,
    /// Overrides du nœud — littéral OU `{{ msg.x }}` (composition 2 niveaux).
    #[serde(default)]
    vars: BTreeMap<String, String>,
    /// Snapshot des canaux (résolu au deploy, clés estampées par le
    /// backend) — **requis** : un artefact sans snapshot est un artefact
    /// périmé (refus explicite au build).
    #[serde(rename = "pnex_notify_channels")]
    channels: Vec<ChannelSnapshot>,
    /// Snapshot du template — **requis** au même titre.
    #[serde(rename = "pnex_notify_template")]
    template: TemplateSnapshot,
    /// Anti-spam — fenêtre fixe « max N envois / fenêtre de D secondes » :
    /// le premier envoi ouvre la fenêtre, le surplus est bloqué (warn,
    /// l'envoi est différé — pas perdu — quand des vars sont accumulées ;
    /// jeté en mode immédiat). `None` = pas de limite.
    #[serde(default)]
    anti_spam: Option<pnex_core::AntiSpamConfig>,
    /// Boolean gate input, deploy-derived (true iff a wire is annotated on
    /// the `trigger` row): a message arriving with `topic = "trigger"`
    /// arms/disarms the node and gates every send. Absent in old artifacts
    /// — `ensure_trigger_wired` refuses to build them (mandatory gate).
    #[serde(default, rename = "pnex_notify_trigger")]
    trigger: bool,
}

/// État muable du nœud (école vendor join.rs : `Mutex` std, scopes courts,
/// jamais gardé across `.await`) — accumulation des vars (mode ancres
/// nommées) + fenêtre anti-spam.
#[derive(Debug, Default)]
struct NotifyNodeState {
    /// Vars remplies en attente du set complet (mode accumulation).
    filled: BTreeMap<String, serde_json::Value>,
    /// Début de la fenêtre anti-spam courante (None = pas encore ouverte).
    window_start: Option<Instant>,
    /// Envois comptés dans la fenêtre courante.
    sent_in_window: u32,
    /// Trigger gate: data messages only render+send while armed (meaningful
    /// only when the trigger input is wired, `config.trigger`).
    armed: bool,
    /// A complete accumulated set is waiting for the trigger to commit it.
    ready: bool,
    /// Deferred trigger commit: when set, the pending set is rendered and
    /// sent at that instant by the commit task (data messages arriving
    /// meanwhile only refresh the set, they never send on their own).
    commit_at: Option<Instant>,
}

/// Grace window between an armed trigger and the commit of the pending set.
/// Fan-out from one source tick reaches this node within microseconds, so
/// this is only a bound on the extra latency of an alert.
const COMMIT_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// What an accumulation-mode data message leads to.
#[derive(Debug, PartialEq)]
enum DataOutcome {
    /// The stamped set is not complete yet.
    Incomplete,
    /// Complete, but the trigger is disarmed: the set waits (and keeps
    /// being refreshed by later messages).
    Disarmed,
    /// A trigger commit is scheduled: the commit task will send the set.
    AwaitingCommit,
    /// Complete and armed: render + send this context now.
    Send(serde_json::Value),
}

impl NotifyNodeState {
    /// Snapshot of the accumulated vars as a render context.
    fn context(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.filled
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
    }

    /// Applies a trigger message; returns true when a commit was newly
    /// scheduled (the commit task must be woken up).
    fn apply_trigger(&mut self, value: Option<bool>, now: Instant) -> bool {
        let has_pending = !self.filled.is_empty();
        let outcome = trigger_gate_update(&mut self.armed, &mut self.ready, has_pending, value);
        if outcome == TriggerOutcome::Commit && self.commit_at.is_none() {
            self.commit_at = Some(now + COMMIT_GRACE);
            return true;
        }
        false
    }

    /// Applies an accumulation-mode data message (fill, last value wins).
    fn apply_data(
        &mut self,
        stamped: &[String],
        topic: Option<&str>,
        payload: &serde_json::Value,
        wired: bool,
    ) -> DataOutcome {
        if !fill_vars(&mut self.filled, stamped, topic, payload) {
            return DataOutcome::Incomplete;
        }
        if wired {
            self.ready = true;
        }
        if self.commit_at.is_some() {
            return DataOutcome::AwaitingCommit;
        }
        if !trigger_allows_send(self.armed, wired) {
            return DataOutcome::Disarmed;
        }
        DataOutcome::Send(self.context())
    }

    /// Takes a due trigger commit: clears the schedule and returns the
    /// context to send (the latest values), if a complete set is pending.
    fn take_due_commit(&mut self, now: Instant) -> Option<serde_json::Value> {
        match self.commit_at {
            Some(at) if at <= now => self.commit_at = None,
            _ => return None,
        }
        (self.ready && !self.filled.is_empty()).then(|| self.context())
    }
}

#[derive(Debug)]
#[flow_node("pnex-notify", red_name = "pnex-notify")]
struct PnexNotifyNode {
    base: BaseFlowNodeState,
    config: PnexNotifyNodeConfig,
    /// Canaux prêts à l'envoi : (kind, config effective) — les canaux
    /// websocket ont leur config composée ici depuis l'env (une fois).
    resolved: Vec<(String, serde_json::Value)>,
    /// Channel ids, aligned with `resolved` (journal entries).
    channel_ids: Vec<uuid::Uuid>,
    /// Delivery journal endpoint + token (D86) — `None` when the runtime
    /// env lacks them (entries are then only logged).
    journal: Option<(String, String)>,
    /// Vault endpoint + runtime token (D115) — set when a channel holds a
    /// secret reference.
    secret_source: Option<(String, String)>,
    /// Sendable configs resolved from the vault, by channel index.
    secret_cache: Mutex<BTreeMap<usize, serde_json::Value>>,
    /// État muable : accumulation + anti-spam.
    state: Mutex<NotifyNodeState>,
    /// Wakes the commit task when a trigger commit gets scheduled.
    commit_wake: tokio::sync::Notify,
}

impl PnexNotifyNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = PnexNotifyNodeConfig::deserialize(&config.rest).map_err(|e| {
            let hint = if e.to_string().contains("missing field") {
                " — notify snapshot missing: redeploy the flow (D50)"
            } else {
                ""
            };
            EdgelinkError::BadFlowsJson(format!("pnex-notify: invalid config: {e}{hint}"))
        })?;
        ensure_trigger_wired(&cfg)?;
        let mut resolved = Vec::with_capacity(cfg.channels.len());
        let channel_ids = cfg
            .channels
            .iter()
            .map(|ch| uuid::Uuid::parse_str(&ch.id).unwrap_or_default())
            .collect();
        let journal = match (
            std::env::var("PNEX_NOTIFY_JOURNAL_URL"),
            std::env::var("PNEX_NOTIFY_DELIVER_TOKEN"),
        ) {
            (Ok(url), Ok(token)) if !url.is_empty() && !token.is_empty() => Some((url, token)),
            _ => {
                log::warn!(
                    "pnex-notify: PNEX_NOTIFY_JOURNAL_URL/_DELIVER_TOKEN missing from the runtime \
                     environment — deliveries will not be journaled"
                );
                None
            }
        };
        for ch in &cfg.channels {
            if ch.kind == "websocket" {
                // Coordonnées injectées par le supervisor ( FlowSettings
                // .notify_deliver → apply_runtime_env). Jamais en flows.json.
                let url = std::env::var("PNEX_NOTIFY_DELIVER_URL").map_err(|_| {
                    EdgelinkError::InvalidOperation(
                        "pnex-notify: PNEX_NOTIFY_DELIVER_URL missing from the runtime \
                         environment (internal_token not configured on the server?)"
                            .into(),
                    )
                })?;
                let token = std::env::var("PNEX_NOTIFY_DELIVER_TOKEN").map_err(|_| {
                    EdgelinkError::InvalidOperation(
                        "pnex-notify: PNEX_NOTIFY_DELIVER_TOKEN missing from the runtime \
                         environment"
                            .into(),
                    )
                })?;
                resolved.push((
                    "websocket".into(),
                    serde_json::json!({
                        "deliver_url": url,
                        "token": token,
                        "org_id": cfg.pnex_org_id,
                        "channel_id": ch.id,
                    }),
                ));
            } else {
                resolved.push((ch.kind.clone(), ch.config.clone()));
            }
        }
        let needs_vault = resolved
            .iter()
            .any(|(kind, config)| !pnex_notify::secrets::secret_refs(kind, config).is_empty());
        let secret_source =
            if needs_vault {
                let url = std::env::var("PNEX_FLOW_SECRET_URL")
                    .ok()
                    .filter(|v| !v.is_empty());
                let token = std::env::var("PNEX_FLOW_WRITE_TOKEN")
                    .ok()
                    .filter(|v| !v.is_empty());
                match (url, token) {
                    (Some(url), Some(token)) => Some((url, token)),
                    _ => return Err(EdgelinkError::InvalidOperation(
                        "pnex-notify: PNEX_FLOW_SECRET_URL/PNEX_FLOW_WRITE_TOKEN missing from the \
                         runtime environment (runtime_token not configured on the server?)"
                            .into(),
                    )
                    .into()),
                }
            } else {
                None
            };

        Ok(Box::new(PnexNotifyNode {
            base: base_node,
            config: cfg,
            resolved,
            channel_ids,
            journal,
            secret_source,
            secret_cache: Mutex::new(BTreeMap::new()),
            state: Mutex::new(NotifyNodeState::default()),
            commit_wake: tokio::sync::Notify::new(),
        }))
    }

    /// The config of channel `index` with its vault references replaced by
    /// their values (fetched once, then cached until the next deploy).
    async fn sendable(
        &self,
        index: usize,
        kind: &str,
        config: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, pnex_notify::NotifyError> {
        let refs = pnex_notify::secrets::secret_refs(kind, config);
        if refs.is_empty() {
            return Ok(config.clone());
        }
        if let Some(hit) = self
            .secret_cache
            .lock()
            .expect("pnex-notify secret cache")
            .get(&index)
        {
            return Ok(hit.clone());
        }
        let Some((url, token)) = &self.secret_source else {
            return Err(pnex_notify::NotifyError::Config(
                "secret endpoint not configured".into(),
            ));
        };
        let mut values = Vec::with_capacity(refs.len());
        for (field, id) in refs {
            values.push((
                field,
                fetch_secret(url, token, self.config.pnex_org_id, id).await?,
            ));
        }
        let ready = pnex_notify::secrets::with_values(config, &values);
        self.secret_cache
            .lock()
            .expect("pnex-notify secret cache")
            .insert(index, ready.clone());
        Ok(ready)
    }

    /// Graph node id (stamped at deploy), else the runtime name.
    fn node_ref(&self) -> String {
        if self.config.pnex_node_id.is_empty() {
            self.name().to_string()
        } else {
            self.config.pnex_node_id.clone()
        }
    }

    /// Reports one attempt to the delivery journal (detached: never delays
    /// nor fails the send).
    fn journal_attempt(
        &self,
        index: usize,
        msg: &pnex_notify::Message,
        status: &str,
        http_status: Option<u16>,
        error: Option<String>,
    ) {
        let entry = pnex_core::NotifyDeliveryEntry {
            org_id: self.config.pnex_org_id,
            channel_id: self.channel_ids.get(index).copied().unwrap_or_default(),
            channel_kind: self
                .resolved
                .get(index)
                .map(|(k, _)| k.clone())
                .unwrap_or_default(),
            template_id: self
                .config
                .template_id
                .as_deref()
                .and_then(|t| uuid::Uuid::parse_str(t).ok()),
            source: "flow".into(),
            status: status.into(),
            http_status,
            error,
            subject: msg.subject.clone(),
            flow_id: Some(self.config.pnex_flow_id).filter(|f| *f > 0),
            node_id: Some(self.node_ref()).filter(|n| !n.is_empty()),
            delivered: None,
        };
        let Some((url, token)) = self.journal.clone() else {
            log::debug!(
                "pnex-notify [{}] : journal disabled, {status} not reported",
                self.name()
            );
            return;
        };
        let node = self.name().to_string();
        tokio::spawn(async move {
            if let Err(e) = pnex_notify::journal::post(&url, &token, &entry).await {
                log::warn!("pnex-notify [{node}] : journal write failed: {e}");
            }
        });
    }

    /// One incoming message → (anchor mode: accumulation until the set is
    /// complete) → `deliver` → passthrough of the original message (always,
    /// even after a lenient failure). Trigger messages are consumed.
    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        log::debug!("pnex-notify [{}] : message received", self.name());

        // 1) Payload + topic outside the guard (never await under a lock).
        let (payload, topic) = {
            let m = msg.read().await;
            let payload = match m.get("payload").cloned() {
                Some(v) => serde_json::to_value(&v).map_err(|e| {
                    EdgelinkError::InvalidOperation(format!(
                        "pnex-notify [{}] : payload is not serializable: {e}",
                        self.name()
                    ))
                })?,
                None => serde_json::Value::Null,
            };
            let topic = m.get("topic").and_then(|v| v.as_str()).map(str::to_string);
            (payload, topic)
        };

        // 2) Render context. Three paths: a trigger update (gate only, the
        // commit itself is deferred to the commit task), the immediate mode (no
        // template vars: render+send on every message), and accumulation
        // (fill until the stamped set is complete).
        let context_payload =
            if self.config.trigger && topic.as_deref() == Some(pnex_core::NOTIFY_TRIGGER_PIN) {
                // Gate update, never template data: the boolean arms/disarms the
                // node; an armed `true` schedules the commit of a pending
                // complete set. The raw boolean is consumed (no passthrough).
                let boolean = coerce_trigger_bool(&payload);
                let (scheduled, was_armed, armed) = {
                    let mut st = self.state.lock().expect("pnex-notify state");
                    let was_armed = st.armed;
                    (
                        st.apply_trigger(boolean, Instant::now()),
                        was_armed,
                        st.armed,
                    )
                };
                if scheduled {
                    self.commit_wake.notify_one();
                }
                log::debug!(
                    "pnex-notify [{}] : trigger {} (armed={armed}, commit scheduled={scheduled})",
                    self.name(),
                    match boolean {
                        Some(true) => "true",
                        Some(false) => "false",
                        None => "unrecognized (treated as false)",
                    },
                );
                // A template without variables has no data anchor: nothing
                // can fill a set, so the trigger itself is the alert — sent
                // on its rising edge only (O23: a trigger held `true` by a
                // periodic inject must not resend every tick).
                if alert_on_trigger(self.config.template.vars.is_empty(), was_armed, armed) {
                    self.deliver(serde_json::json!({})).await?;
                }
                return Ok(());
            } else if self.config.template.vars.is_empty() {
                // Immediate mode — gated by the armed flag when the trigger input
                // is wired (a disarmed message passes through, unsent).
                if !trigger_allows_send(
                    self.state.lock().expect("pnex-notify state").armed,
                    self.config.trigger,
                ) {
                    log::debug!(
                        "pnex-notify [{}] : message dropped (trigger disarmed)",
                        self.name()
                    );
                    return self.fan_out_one(Envelope { port: 0, msg }, cancel).await;
                }
                payload
            } else {
                // Accumulation: fill (last value wins) until the stamped set is
                // complete; a disarmed or commit-pending set waits — deferred,
                // never lost, always refreshed with the latest values.
                let outcome = self.state.lock().expect("pnex-notify state").apply_data(
                    &self.config.template.vars,
                    topic.as_deref(),
                    &payload,
                    self.config.trigger,
                );
                match outcome {
                    DataOutcome::Send(ctx) => ctx,
                    DataOutcome::Incomplete => {
                        return self.fan_out_one(Envelope { port: 0, msg }, cancel).await;
                    }
                    DataOutcome::Disarmed | DataOutcome::AwaitingCommit => {
                        log::debug!(
                            "pnex-notify [{}] : complete set waiting for the trigger ({outcome:?})",
                            self.name()
                        );
                        return self.fan_out_one(Envelope { port: 0, msg }, cancel).await;
                    }
                }
            };

        self.deliver(context_payload).await?;
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }

    /// Render → anti-spam gate → send on every channel → reset of the
    /// accumulated set. `Err` only in strict mode (render or send failure);
    /// a lenient failure or an anti-spam block keeps the set pending.
    async fn deliver(&self, context_payload: serde_json::Value) -> Result<()> {
        let meta = serde_json::json!({
            "flow": self.config.pnex_flow_id,
            "node": self.node_ref(),
            "template": self.config.template_id,
            "ts": chrono::Utc::now().to_rfc3339(),
            "org": self.config.pnex_org_id,
        });

        // 1) Render (once, shared by every channel).
        let rendered = match pnex_notify::render(
            self.config.template.subject.as_deref(),
            &self.config.template.body,
            &self.config.vars,
            context_payload,
            meta,
        ) {
            Ok(m) => m,
            Err(e) => {
                if self.config.strict {
                    return Err(EdgelinkError::InvalidOperation(format!(
                        "pnex-notify [{}] : render failed: {e}",
                        self.name()
                    ))
                    .into());
                }
                log::warn!(
                    "pnex-notify [{}] : render failed (lenient): {e}",
                    self.name()
                );
                return Ok(());
            }
        };

        // 2) Anti-spam gate (fixed "count × duration" window): a blocked send
        // is a warn — never an error, even in strict mode (blocking is the
        // node's job, not a failure). In accumulation mode the set stays
        // pending: the send is deferred, not lost.
        if let Some(anti) = self.config.anti_spam {
            let allowed = {
                let mut st = self.state.lock().expect("pnex-notify state");
                let NotifyNodeState {
                    window_start,
                    sent_in_window,
                    ..
                } = &mut *st;
                anti_spam_gate(window_start, sent_in_window, anti, Instant::now())
            };
            if !allowed {
                log::warn!(
                    "pnex-notify [{}] : send blocked (anti-spam max {} / {} s)",
                    self.name(),
                    anti.max_msgs,
                    anti.window_secs
                );
                for i in 0..self.resolved.len() {
                    self.journal_attempt(
                        i,
                        &rendered,
                        pnex_core::DELIVERY_BLOCKED,
                        None,
                        Some(format!(
                            "anti-spam: max {} / {} s",
                            anti.max_msgs, anti.window_secs
                        )),
                    );
                }
                return Ok(());
            }
        }

        // 3) Send per channel (at-least-once retry, D53). Lenient: each
        // failure is a warn, the flow goes on; strict: first error rejected
        // (isolated flow_error).
        for (i, (kind, config)) in self.resolved.iter().enumerate() {
            let Some(ch) = pnex_notify::channel(kind) else {
                log::warn!(
                    "pnex-notify [{}] : kind \"{}\" unknown to the registry (runtime \
                    older than the server?)",
                    self.name(),
                    kind
                );
                continue;
            };
            let outcome = match self.sendable(i, kind, config).await {
                Ok(config) => pnex_notify::with_retry(|| ch.send(&config, &rendered)).await,
                Err(e) => Err(e),
            };
            // The websocket success is journaled by the backend deliver
            // endpoint; everything else is reported from here.
            match &outcome {
                Ok(()) if kind != "websocket" => {
                    self.journal_attempt(i, &rendered, pnex_core::DELIVERY_SENT, None, None)
                }
                Ok(()) => {}
                Err(e) => self.journal_attempt(
                    i,
                    &rendered,
                    pnex_core::DELIVERY_FAILED,
                    pnex_notify::journal::http_status(e),
                    Some(e.to_string()),
                ),
            }
            match outcome {
                Ok(()) => {
                    log::debug!("pnex-notify [{}] : sent on \"{kind}\"", self.name());
                }
                Err(e) => {
                    if self.config.strict {
                        return Err(EdgelinkError::InvalidOperation(format!(
                            "pnex-notify [{}] : send on \"{kind}\" failed: {e}",
                            self.name()
                        ))
                        .into());
                    }
                    log::warn!(
                        "pnex-notify [{}] : send on \"{kind}\" failed (lenient): {e}",
                        self.name()
                    );
                }
            }
        }

        // 4) Set sent: reset the accumulation (the anti-spam window survives).
        if !self.config.template.vars.is_empty() {
            let mut st = self.state.lock().expect("pnex-notify state");
            st.filled.clear();
            st.ready = false;
        }
        Ok(())
    }

    /// Commit task (one per node, alongside the message loop): waits for a
    /// scheduled trigger commit, then renders and sends the latest values
    /// of the pending set once the grace window has elapsed.
    async fn commit_loop(self: Arc<Self>, stop: CancellationToken) {
        loop {
            let due = self.state.lock().expect("pnex-notify state").commit_at;
            match due {
                None => tokio::select! {
                    _ = stop.cancelled() => return,
                    _ = self.commit_wake.notified() => continue,
                },
                Some(at) => tokio::select! {
                    _ = stop.cancelled() => return,
                    _ = tokio::time::sleep_until(tokio::time::Instant::from_std(at)) => {}
                },
            }
            let ctx = self
                .state
                .lock()
                .expect("pnex-notify state")
                .take_due_commit(Instant::now());
            if let Some(ctx) = ctx {
                // No message to reject here: a strict failure is only logged.
                if let Err(e) = self.deliver(ctx).await {
                    log::warn!("pnex-notify [{}] : trigger commit failed: {e}", self.name());
                }
            }
        }
    }
}

/// Fills `filled` from an incoming message: topic = stamped var ⇒ the whole
/// payload fills that var; otherwise the keys of an object payload ∩ stamped
/// vars fill their vars. **Last value wins**: a set waiting for the trigger
/// or for the anti-spam window always renders the most recent data, never a
/// value frozen at the time the set first became complete. Returns true when
/// the stamped set is complete.
/// One vault secret, retried briefly on 404: right after a first deploy
/// the backend may not have marked the flow deployed yet.
async fn fetch_secret(
    url: &str,
    token: &str,
    org_id: i64,
    id: uuid::Uuid,
) -> std::result::Result<String, pnex_notify::NotifyError> {
    let mut attempt = 0;
    loop {
        match pnex_notify::secrets::fetch(url, token, org_id, id).await {
            Err(pnex_notify::NotifyError::Http { status: 404 }) if attempt < 3 => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt)).await;
            }
            other => {
                return other.map_err(|e| match e {
                    pnex_notify::NotifyError::Http { status: 404 } => {
                        pnex_notify::NotifyError::Config(
                            "secret not available: channel disabled or not referenced by a \
                             deployed flow"
                                .into(),
                        )
                    }
                    e => e,
                })
            }
        }
    }
}

fn fill_vars(
    filled: &mut BTreeMap<String, serde_json::Value>,
    stamped: &[String],
    topic: Option<&str>,
    payload: &serde_json::Value,
) -> bool {
    let mut fill = |name: &str, value: serde_json::Value| {
        filled.insert(name.to_string(), value);
    };
    match topic {
        Some(t) if stamped.iter().any(|v| v == t) => fill(t, payload.clone()),
        _ => {
            if let serde_json::Value::Object(map) = payload {
                for (k, v) in map {
                    if stamped.iter().any(|name| name == k) {
                        fill(k, v.clone());
                    }
                }
            }
        }
    }
    stamped.iter().all(|v| filled.contains_key(v))
}

/// Porte anti-spam fenêtre fixe : true = envoi autorisé. Le premier envoi
/// ouvre la fenêtre ; `max_msgs` envois y sont autorisés, ensuite refusé
/// jusqu'à expiration (`window_secs`), puis la fenêtre se rouvre.
fn anti_spam_gate(
    window_start: &mut Option<Instant>,
    sent_in_window: &mut u32,
    cfg: pnex_core::AntiSpamConfig,
    now: Instant,
) -> bool {
    let expired = match *window_start {
        Some(start) => now.duration_since(start).as_secs() >= cfg.window_secs,
        None => true,
    };
    if expired {
        *window_start = Some(now);
        *sent_in_window = 1;
        return true;
    }
    if *sent_in_window < cfg.max_msgs {
        *sent_in_window += 1;
        return true;
    }
    false
}

/// What the node does with a message arriving on the `trigger` topic.
#[derive(Debug, PartialEq, Eq)]
enum TriggerOutcome {
    /// An armed `true` committed a pending complete set: render + send the
    /// accumulated vars now.
    Commit,
    /// The gate was updated only — the message is consumed (no passthrough:
    /// the raw boolean never reaches downstream nodes).
    Consumed,
}

/// Applies one trigger message to the gate state: `value` arms/disarms the
/// node (an unrecognized payload disarms); an armed `true` commits a pending
/// complete set (`ready` with accumulated vars) — the wired logic function
/// decides whether the notification is sent.
fn trigger_gate_update(
    armed: &mut bool,
    ready: &mut bool,
    has_pending: bool,
    value: Option<bool>,
) -> TriggerOutcome {
    *armed = value.unwrap_or(false);
    if *armed && *ready && has_pending {
        TriggerOutcome::Commit
    } else {
        TriggerOutcome::Consumed
    }
}

/// Variable-free template: the alert fires on the trigger's rising edge
/// (false/unrecognized → true); a `false` re-arms it for the next edge.
fn alert_on_trigger(vars_empty: bool, was_armed: bool, armed: bool) -> bool {
    vars_empty && armed && !was_armed
}

/// Mandatory trigger gate: a snapshot without the deploy-derived flag is a
/// stale artifact of the always-send era — refused loudly (D50 school,
/// isolated flow_error) so no unsolicited notification can ever go out.
/// The fix is a redeploy with the trigger input wired.
fn ensure_trigger_wired(cfg: &PnexNotifyNodeConfig) -> Result<()> {
    if cfg.trigger {
        return Ok(());
    }
    Err(EdgelinkError::BadFlowsJson(
        "pnex-notify: the trigger input is not wired — wire a boolean logic-function output \
         to the trigger anchor (mandatory anti-spam gate) and redeploy the flow"
            .into(),
    )
    .into())
}

/// Coerces a trigger payload to its boolean gate value (Node-RED style
/// truthiness): JSON booleans pass through, numbers are true when non-zero,
/// the strings `true`/`false`/`1`/`0` are accepted case-insensitively;
/// anything else disarms (fail-safe).
fn coerce_trigger_bool(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::Number(n) => Some(n.as_f64().is_some_and(|f| f != 0.0)),
        serde_json::Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Gate for data messages: messages only render+send while the trigger is
/// armed. The unwired case (`wired = false`, deploy flag absent) cannot
/// reach the runtime anymore — `ensure_trigger_wired` refuses it at build —
/// but the helper keeps the unwired-passes behavior for tests and defense.
fn trigger_allows_send(armed: bool, wired: bool) -> bool {
    !wired || armed
}

#[async_trait]
impl FlowNodeBehavior for PnexNotifyNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        let committer = tokio::spawn(self.clone().commit_loop(stop_token.clone()));
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &PnexNotifyNode, msg: MsgHandle| async move {
                    // `with_uow` route les erreurs vers `flow.handle_error` sans
                    // les logger : on journalise ici pour l'exploitation.
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("pnex-notify [{}] : message rejected: {e}", node.name());
                            Err(e)
                        }
                    }
                },
            )
            .await;
        }
        committer.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgelink_core::runtime::registry::RegistryBuilder;

    #[test]
    fn node_enregistre_dans_le_registre() {
        let reg = RegistryBuilder::default().build().expect("registre");
        let meta = reg
            .get("pnex-notify")
            .expect("nœud pnex-notify absent du registre");
        assert_eq!(meta.type_, "pnex-notify");
    }

    #[test]
    fn registered_ne_panique_pas() {
        registered();
    }

    /// L'artefact périmé (sans snapshot) est refusé avec un message
    /// explicite « redéployez » — jamais de nœud silencieux sans canaux.
    #[test]
    fn config_rejete_snapshot_manquant() {
        let err = serde_json::from_str::<PnexNotifyNodeConfig>(
            r#"{"channel_ids":["x"],"template_id":null}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing field"), "{err}");
    }

    #[test]
    fn config_accepte_snapshot_complet() {
        let cfg: PnexNotifyNodeConfig = serde_json::from_str(
            r#"{
                "pnex_flow_id": 12, "pnex_org_id": 7, "strict": true,
                "vars": {"seuil": "{{ msg.value }}"},
                "pnex_notify_channels": [{"id": "3f2e", "kind": "websocket"}],
                "pnex_notify_template": {"subject": "[{{ device }}]", "body": "{{ value }}"},
                "anti_spam": {"max_msgs": 3, "window_secs": 600}
            }"#,
        )
        .expect("snapshot complet désérialisable");
        assert!(cfg.strict);
        assert_eq!(cfg.channels.len(), 1);
        assert_eq!(cfg.template.body, "{{ value }}");
        // Nouveautés : vars du snapshot (mode accumulation) + anti-spam.
        assert!(cfg.template.vars.is_empty());
        assert_eq!(
            cfg.anti_spam,
            Some(pnex_core::AntiSpamConfig {
                max_msgs: 3,
                window_secs: 600
            })
        );
        // Old artifacts carry no trigger flag — refused at build (see
        // `ensure_trigger_wired`), the deserializer itself stays tolerant.
        assert!(!cfg.trigger);
    }

    #[test]
    fn build_refuse_trigger_absent() {
        // Anti-spam guarantee: no trigger flag = always-send era artifact —
        // never built, so the node can never send unsolicited notifications.
        let cfg: PnexNotifyNodeConfig = serde_json::from_str(
            r#"{
                "pnex_notify_channels": [],
                "pnex_notify_template": {"body": "x"}
            }"#,
        )
        .expect("snapshot without flag deserializable");
        let err = ensure_trigger_wired(&cfg).unwrap_err();
        assert!(
            err.to_string().contains("trigger input is not wired"),
            "{err}"
        );
    }

    #[test]
    fn config_accepte_flag_trigger() {
        let cfg: PnexNotifyNodeConfig = serde_json::from_str(
            r#"{
                "pnex_notify_channels": [],
                "pnex_notify_template": {"body": "x"},
                "pnex_notify_trigger": true
            }"#,
        )
        .expect("trigger flag deserializable");
        assert!(cfg.trigger);
    }

    #[test]
    fn trigger_gate_arme_desarme_et_commit() {
        let mut armed = false;
        let mut ready = false;
        // Disarmed by default: a true arms the gate, nothing pending yet.
        assert_eq!(
            trigger_gate_update(&mut armed, &mut ready, false, Some(true)),
            TriggerOutcome::Consumed
        );
        assert!(armed);
        // A false disarms.
        assert_eq!(
            trigger_gate_update(&mut armed, &mut ready, false, Some(false)),
            TriggerOutcome::Consumed
        );
        assert!(!armed);
        // Pending set + armed true → commit (the logic function decides).
        ready = true;
        assert_eq!(
            trigger_gate_update(&mut armed, &mut ready, true, Some(true)),
            TriggerOutcome::Commit
        );
        assert!(armed);
        // Disarmed trigger never commits, even with a pending set.
        assert_eq!(
            trigger_gate_update(&mut armed, &mut ready, true, Some(false)),
            TriggerOutcome::Consumed
        );
        // Non-boolean payload disarms (fail-safe), never commits.
        assert_eq!(
            trigger_gate_update(&mut armed, &mut ready, true, None),
            TriggerOutcome::Consumed
        );
        assert!(!armed);
    }

    #[test]
    fn trigger_gate_permet_envoi_seulement_arme() {
        // Unwired: immediate always-send.
        assert!(trigger_allows_send(false, false));
        assert!(trigger_allows_send(true, false));
        // Wired: only an armed gate lets data messages through.
        assert!(!trigger_allows_send(false, true));
        assert!(trigger_allows_send(true, true));
    }

    #[test]
    fn coerce_trigger_bool_style_node_red() {
        assert_eq!(coerce_trigger_bool(&serde_json::json!(true)), Some(true));
        assert_eq!(coerce_trigger_bool(&serde_json::json!(false)), Some(false));
        // Numbers: Node-RED truthiness — non-zero arms, zero disarms.
        assert_eq!(coerce_trigger_bool(&serde_json::json!(1)), Some(true));
        assert_eq!(coerce_trigger_bool(&serde_json::json!(0)), Some(false));
        assert_eq!(coerce_trigger_bool(&serde_json::json!(-2.5)), Some(true));
        // Strings, case-insensitive, trimmed.
        assert_eq!(coerce_trigger_bool(&serde_json::json!("TRUE")), Some(true));
        assert_eq!(coerce_trigger_bool(&serde_json::json!(" 0 ")), Some(false));
        assert_eq!(coerce_trigger_bool(&serde_json::json!("on")), None);
        // Anything else disarms (fail-safe).
        assert_eq!(coerce_trigger_bool(&serde_json::json!({"a": 1})), None);
        assert_eq!(coerce_trigger_bool(&serde_json::json!(null)), None);
    }

    #[test]
    fn snapshot_accepte_vars_template() {
        let cfg: PnexNotifyNodeConfig = serde_json::from_str(
            r#"{
                "pnex_notify_channels": [],
                "pnex_notify_template": {"body": "x", "vars": ["seuil", "value"]}
            }"#,
        )
        .expect("vars de snapshot désérialisables");
        assert_eq!(
            cfg.template.vars,
            vec!["seuil".to_string(), "value".to_string()]
        );
    }

    #[test]
    fn fill_par_topic_puis_par_cles_payload() {
        let stamped = vec!["seuil".to_string(), "value".to_string()];
        let mut filled = BTreeMap::new();
        // Tagger : topic = var, payload entier = valeur de la var.
        assert!(!fill_vars(
            &mut filled,
            &stamped,
            Some("seuil"),
            &serde_json::json!(80),
        ));
        // Payload objet : les clés stampées remplissent leurs vars.
        assert!(fill_vars(
            &mut filled,
            &stamped,
            None,
            &serde_json::json!({"value": 82.5, "ignore": true}),
        ));
        assert_eq!(filled["seuil"], serde_json::json!(80));
        assert_eq!(filled["value"], serde_json::json!(82.5));
        // Last value wins: a refill overwrites (the set stays complete).
        assert!(fill_vars(
            &mut filled,
            &stamped,
            Some("value"),
            &serde_json::json!(99)
        ));
        assert_eq!(filled["value"], serde_json::json!(99));
    }

    /// Replays trigger values through the gate; counts variable-free alerts.
    fn variable_free_alerts(seq: &[Option<bool>]) -> usize {
        let (mut armed, mut ready) = (false, false);
        seq.iter()
            .filter(|v| {
                let was_armed = armed;
                trigger_gate_update(&mut armed, &mut ready, false, **v);
                alert_on_trigger(true, was_armed, armed)
            })
            .count()
    }

    #[test]
    fn variable_free_template_alerts_on_rising_edge_only() {
        // O23: an inject holding the trigger true must not resend each tick.
        assert_eq!(variable_free_alerts(&[Some(true); 3]), 1);
        assert_eq!(
            variable_free_alerts(&[Some(true), Some(false), Some(true)]),
            2
        );
        // An unrecognized payload disarms, so the next true is a new edge.
        assert_eq!(variable_free_alerts(&[Some(true), None, Some(true)]), 2);
        assert_eq!(variable_free_alerts(&[Some(false), Some(false)]), 0);
        // Templates with variables never alert from the trigger alone.
        assert!(!alert_on_trigger(false, false, true));
    }

    fn vars(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn disarmed_set_renders_latest_values_on_commit() {
        // O10: trigger false → complete set → newer values → trigger true →
        // the committed context carries the newest values, not the first.
        let stamped = vars(&["eta"]);
        let mut st = NotifyNodeState::default();
        let t0 = Instant::now();
        assert!(!st.apply_trigger(Some(false), t0));
        assert_eq!(
            st.apply_data(&stamped, Some("eta"), &serde_json::json!(0), true),
            DataOutcome::Disarmed
        );
        assert_eq!(
            st.apply_data(&stamped, Some("eta"), &serde_json::json!(17), true),
            DataOutcome::Disarmed
        );
        assert!(st.apply_trigger(Some(true), t0));
        assert_eq!(
            st.take_due_commit(t0),
            None,
            "not due before the grace window"
        );
        assert_eq!(
            st.take_due_commit(t0 + COMMIT_GRACE),
            Some(serde_json::json!({"eta": 17}))
        );
        // The schedule is consumed.
        assert_eq!(st.take_due_commit(t0 + COMMIT_GRACE), None);
    }

    #[test]
    fn trigger_before_data_commits_the_same_tick_values() {
        // O11: pending set from tick t-1, then trigger(t) arrives before
        // data(t): the commit waits the grace window and sends data(t).
        let stamped = vars(&["vibration"]);
        let mut st = NotifyNodeState::default();
        let t0 = Instant::now();
        st.apply_trigger(Some(false), t0);
        st.apply_data(&stamped, Some("vibration"), &serde_json::json!(2.06), true);
        assert!(st.apply_trigger(Some(true), t0));
        // data(t) arrives while the commit is pending: refresh only, no send.
        assert_eq!(
            st.apply_data(&stamped, Some("vibration"), &serde_json::json!(10.71), true),
            DataOutcome::AwaitingCommit
        );
        assert_eq!(
            st.take_due_commit(t0 + COMMIT_GRACE),
            Some(serde_json::json!({"vibration": 10.71}))
        );
    }

    #[test]
    fn data_before_trigger_still_commits() {
        let stamped = vars(&["vibration"]);
        let mut st = NotifyNodeState::default();
        let t0 = Instant::now();
        st.apply_trigger(Some(false), t0);
        st.apply_data(&stamped, Some("vibration"), &serde_json::json!(11), true);
        assert!(st.apply_trigger(Some(true), t0));
        // A second trigger during the grace window does not reschedule.
        assert!(!st.apply_trigger(Some(true), t0 + COMMIT_GRACE / 2));
        assert_eq!(
            st.take_due_commit(t0 + COMMIT_GRACE),
            Some(serde_json::json!({"vibration": 11}))
        );
    }

    #[test]
    fn armed_complete_set_sends_immediately() {
        // Nothing pending when the trigger arms: the next complete set sends
        // right away (no commit scheduled).
        let stamped = vars(&["a", "b"]);
        let mut st = NotifyNodeState::default();
        let t0 = Instant::now();
        assert!(!st.apply_trigger(Some(true), t0));
        assert_eq!(
            st.apply_data(&stamped, Some("a"), &serde_json::json!(1), true),
            DataOutcome::Incomplete
        );
        assert_eq!(
            st.apply_data(&stamped, Some("b"), &serde_json::json!(2), true),
            DataOutcome::Send(serde_json::json!({"a": 1, "b": 2}))
        );
    }

    #[test]
    fn anti_spam_deferred_set_keeps_refreshing() {
        // A set blocked by the anti-spam stays pending (not cleared) and is
        // refreshed by later values: once the window expires the next send
        // carries the latest data.
        let stamped = vars(&["v"]);
        let mut st = NotifyNodeState::default();
        let t0 = Instant::now();
        st.apply_trigger(Some(true), t0);
        assert!(matches!(
            st.apply_data(&stamped, Some("v"), &serde_json::json!(1), true),
            DataOutcome::Send(_)
        ));
        // Blocked by the anti-spam: `deliver` does not clear the set.
        assert_eq!(
            st.apply_data(&stamped, Some("v"), &serde_json::json!(5), true),
            DataOutcome::Send(serde_json::json!({"v": 5}))
        );
    }

    #[test]
    fn fill_immediate_mode_complete_on_first_msg() {
        // Sans var stampée, pas d'accumulation (le mode immédiat est géré en
        // amont par la branche vars.is_empty() — ici on vérifie que fill
        // sur un set vide est trivialement complet).
        let mut filled = BTreeMap::new();
        assert!(fill_vars(
            &mut filled,
            &[],
            Some("x"),
            &serde_json::json!(1)
        ));
    }

    #[test]
    fn fill_payload_non_objet_sans_topic_ne_remplit_rien() {
        let stamped = vec!["a".to_string()];
        let mut filled = BTreeMap::new();
        assert!(!fill_vars(
            &mut filled,
            &stamped,
            None,
            &serde_json::json!(42)
        ));
        assert!(filled.is_empty());
    }

    #[test]
    fn anti_spam_fenetre_fixe() {
        let cfg = pnex_core::AntiSpamConfig {
            max_msgs: 3,
            window_secs: 600,
        };
        let mut start = None;
        let mut count = 0;
        let t0 = Instant::now();
        // Fenêtre fraîche : max envois autorisés.
        assert!(anti_spam_gate(&mut start, &mut count, cfg, t0));
        assert!(anti_spam_gate(&mut start, &mut count, cfg, t0));
        assert!(anti_spam_gate(&mut start, &mut count, cfg, t0));
        // 4e dans la fenêtre : bloqué.
        assert!(!anti_spam_gate(&mut start, &mut count, cfg, t0));
        // Après expiration : la fenêtre se rouvre (compteur reset).
        assert!(anti_spam_gate(
            &mut start,
            &mut count,
            cfg,
            t0 + std::time::Duration::from_secs(600)
        ));
        assert_eq!(count, 1);
    }

    #[test]
    fn anti_spam_premier_envoi_ouvre_la_fenetre() {
        let cfg = pnex_core::AntiSpamConfig {
            max_msgs: 2,
            window_secs: 60,
        };
        let mut start = None;
        let mut count = 0;
        // Instant::now() « loin » dans le futur d'un start=None : aucune
        // expiration possible, la fenêtre s'ouvre au 1er appel.
        let t0 = Instant::now();
        assert!(anti_spam_gate(&mut start, &mut count, cfg, t0));
        assert_eq!(start, Some(t0));
        assert_eq!(count, 1);
    }
}
