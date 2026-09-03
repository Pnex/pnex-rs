//! Protocole fil du canal device — WS `/ws/device` (Brick 0, `docs/architecture/brick0.md` §3).
//!
//! **Source de vérité du contrat** : ce fichier. Le firmware C++
//! (`firmware/generic_esp8266`) en est un **miroir** — l'ESP8266 ne compile
//! pas de Rust : le contrat est le schéma fil (tag `"t"`), pas le partage
//! de code.
//!
//! Framing identique à `/ws/sensor/ingest` — auth query base64
//! (`token` + `device_id`), frames texte `base64(nonce(12)‖ChaCha20-nu)`,
//! `PING`/`PONG` au niveau frame, messages métier JSON tagué `t`.
//!
//! Sémantique RPC à la ThingsBoard : toute commande serveur→device est un
//! RPC avec `cmd_id` et réponse `Ack{cmd_id, ok, err}` requise. La pin map
//! est poussée par le serveur (`ProvisionAck`), jamais déclarée dans le
//! firmware.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Modes P0 d'une capability instance — le rôle sensor/actuator se dérive du
/// mode (pas de colonne `role` : modèle sans copies, B0.6).
/// `digital_in`/`adc_in` → sensor ; `digital_out` → actuator. ⚠ Le fil
/// sérialise `AdcIn` → `"adc_in"` (serde snake_case) — la chaîne
/// `analog_in` est la convention **base** (colonnes `mode`), jamais le fil.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    DigitalIn,
    DigitalOut,
    AdcIn,
}

/// État de repos d'une sortie — appliqué à la perte de lien, au boot et à
/// l'admission (safe-states, brick0.md §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeState {
    Low,
    High,
}

/// Options de configuration d'un pin, poussées dans `SetMode`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeOpts {
    /// Pull-up interne (refusée sur GPIO16 — pulldown only, `caps::validate`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pullup: Option<bool>,
    /// État de repos ; défaut serveur = `Low` si absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_state: Option<SafeState>,
}



/// Pin admis, poussé au device dans `ProvisionAck` (miroir firmware : la
/// carte de pins vient du serveur, jamais du `.bin`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinSpec {
    pub gpio: u16,
    /// Label overlay (« D1 », « A0 ») — dénormalisé pour l'affichage.
    pub label: String,
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_state: Option<SafeState>,
}

/// Device → serveur.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum DeviceMsg {
    /// Premier message après connexion : admission policy `Validated`
    /// (dérive overlay → `caps::validate` → persiste → `ProvisionAck`).
    Announce {
        /// « esp8266 » (P0 — seul chip supporté).
        chip: String,
        /// Board déclaré par le device (« nodemcu », « d1_mini »…) —
        /// informatif P0, la carte réelle = overlay du device en base.
        board: String,
        /// Version du firmware générique (politique de re-flash, §10).
        fw: String,
    },
    /// Lecture périodique d'un pin input (ou réponse à un read).
    StateReport { gpio: u16, value: Value },
    /// Accusé d'un RPC serveur (`cmd_id` requis — sémantique ThingsBoard).
    Ack { cmd_id: String, ok: bool, err: Option<String> },
}

/// Serveur → device.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Réponse à l'`Announce` : pin map complète à appliquer (modes initiaux
    /// + safe-states). Remplace la déclaration statique du firmware.
    ProvisionAck { caps: Vec<PinSpec> },
    /// RPC : changer le mode d'un pin.
    SetMode { cmd_id: String, gpio: u16, mode: Mode, #[serde(default)] opts: ModeOpts },
    /// RPC : écrire sur une sortie (`value` = true/false en P0).
    Write { cmd_id: String, gpio: u16, value: Value },
    /// RPC : cadencer les lectures d'un pin input (0 = désabonner).
    Subscribe { cmd_id: String, gpio: u16, interval_ms: u32 },
    /// Refus d'admission ou erreur fatale — suivi d'une close frame.
    Reject { reason: String },
}

/// Résolution du rôle à partir du mode — sensor/actuator est un rôle dérivé,
/// jamais une colonne (B0.6, modèle sans copies).
pub fn role_of(mode: Mode) -> &'static str {
    match mode {
        Mode::DigitalIn | Mode::AdcIn => "sensor",
        Mode::DigitalOut => "actuator",
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_messages_taguees_t() {
        let m: DeviceMsg = serde_json::from_str(
            r#"{"t":"announce","chip":"esp8266","board":"nodemcu","fw":"0.1.0"}"#,
        )
        .unwrap();
        assert!(matches!(m, DeviceMsg::Announce { chip, board, fw }
            if chip == "esp8266" && board == "nodemcu" && fw == "0.1.0"));
    }
    #[test]
    fn state_report_et_ack_roundtrip() {
        let m: DeviceMsg = serde_json::from_str(
            r#"{"t":"state_report","gpio":5,"value":true}"#,
        )
        .unwrap();
        assert!(matches!(m, DeviceMsg::StateReport { gpio: 5, value }
            if value == Value::Bool(true)));

        let m: DeviceMsg = serde_json::from_str(
            r#"{"t":"ack","cmd_id":"abc","ok":false,"err":"pin non configuré"}"#,
        )
        .unwrap();
        assert!(matches!(m, DeviceMsg::Ack { cmd_id, ok: false, .. }
            if cmd_id == "abc"));
    }

    #[test]
    fn server_msgs_roundtrip() {
        let m: ServerMsg = serde_json::from_str(
            r#"{"t":"set_mode","cmd_id":"c1","gpio":5,"mode":"digital_out"}"#,
        )
        .unwrap();
        assert!(matches!(m, ServerMsg::SetMode { cmd_id: _, gpio: 5, mode: Mode::DigitalOut, opts }
            if opts == ModeOpts::default()));

        let m: ServerMsg = serde_json::from_str(
            r#"{"t":"write","cmd_id":"c2","gpio":5,"value":true}"#,
        )
        .unwrap();
        assert!(matches!(m, ServerMsg::Write { cmd_id, gpio: 5, value }
            if cmd_id == "c2" && value == Value::Bool(true)));
    }
    #[test]
    fn provision_ack_et_reject_roundtrip() {
        let m: ServerMsg = serde_json::from_str(
            r#"{"t":"provision_ack","caps":[{"gpio":5,"label":"D1","mode":"digital_out","safe_state":"low"}]}"#,
        )
        .unwrap();
        assert!(matches!(m, ServerMsg::ProvisionAck { caps }
            if caps.len() == 1
                && caps[0].gpio == 5
                && caps[0].label == "D1"
                && caps[0].mode == Mode::DigitalOut
                && caps[0].safe_state == Some(SafeState::Low)));

        let m: ServerMsg = serde_json::from_str(
            r#"{"t":"reject","reason":"device inconnu"}"#,
        )
        .unwrap();
        assert!(matches!(m, ServerMsg::Reject { reason }
            if reason == "device inconnu"));
    }

    #[test]
    fn role_of_derive_le_role_du_mode() {
        assert_eq!(role_of(Mode::DigitalIn), "sensor");
        assert_eq!(role_of(Mode::AdcIn), "sensor");
        assert_eq!(role_of(Mode::DigitalOut), "actuator");
    }
}
