//! Acknowledgement tracking of the actuation RPCs pushed to a device
//! session (`Write`, `Command`): the device answers `Ack{cmd_id}`.
//!
//! Before this, the push was fire-and-forget: a command lost between the
//! WebSocket push and the device handler left no trace (custom-firmware.md
//! §14.6, 2026-10-06). The session now remembers each pushed command,
//! logs its acknowledgement, warns when none comes back within
//! [`ACK_TIMEOUT`], and pushes an unacknowledged `Write` once more — a pin
//! write sets an absolute value, so a duplicate is harmless. A custom
//! `Command` is never pushed twice: nothing tells whether it is idempotent
//! (a button press would fire twice) and the PneX lib does not deduplicate
//! `cmd_id`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use pnex_core::ServerMsg;

/// Delay after which a command without `Ack` is reported (and a `Write`
/// pushed again). The median command → state round trip is ~180 ms.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(3);

/// Upper bound of tracked commands per session: a device that never acks
/// (legacy firmware) must not grow the map without limit.
const MAX_PENDING: usize = 64;

#[derive(Debug, Clone)]
struct Pending {
    /// Short description for the logs (`write gpio 4`, `command power`).
    what: String,
    /// Plain frame, kept only for a command that may be pushed again.
    resend: Option<String>,
    sent_at: Instant,
    /// Already pushed a second time.
    retried: bool,
}

/// What the session must do for an overdue command.
#[derive(Debug, PartialEq)]
pub enum Overdue {
    /// Push this plain frame again (first timeout of a `Write`).
    Resend {
        cmd_id: String,
        what: String,
        frame: String,
    },
    /// Report the loss (`Command`, or a `Write` already retried).
    Lost { cmd_id: String, what: String },
}

/// Commands pushed in one device session and not yet acknowledged.
#[derive(Debug, Default)]
pub struct PendingAcks {
    pending: HashMap<String, Pending>,
}

impl PendingAcks {
    /// Remembers an actuation pushed as `frame` (other messages ignored).
    pub fn track(&mut self, msg: &ServerMsg, frame: &str, now: Instant) {
        let (cmd_id, what, resend) = match msg {
            ServerMsg::Write { cmd_id, gpio, .. } => (
                cmd_id,
                format!("write gpio {gpio}"),
                Some(frame.to_string()),
            ),
            ServerMsg::Command { cmd_id, name, .. } => (cmd_id, format!("command {name}"), None),
            _ => return,
        };
        if self.pending.len() >= MAX_PENDING {
            // Drop the oldest entry: tracking is best effort.
            if let Some(oldest) = self
                .pending
                .iter()
                .min_by_key(|(_, p)| p.sent_at)
                .map(|(id, _)| id.clone())
            {
                self.pending.remove(&oldest);
            }
        }
        self.pending.insert(
            cmd_id.clone(),
            Pending {
                what,
                resend,
                sent_at: now,
                retried: false,
            },
        );
    }

    /// The device acknowledged `cmd_id`: returns what it was and how long
    /// the round trip took (`None` = not tracked, e.g. a mode change).
    pub fn ack(&mut self, cmd_id: &str, now: Instant) -> Option<(String, Duration)> {
        self.pending
            .remove(cmd_id)
            .map(|p| (p.what, now.saturating_duration_since(p.sent_at)))
    }

    /// Commands overdue at `now`. A `Write` is resent once (and tracked
    /// again); anything else is reported lost and forgotten.
    pub fn overdue(&mut self, now: Instant) -> Vec<Overdue> {
        let late: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| now.saturating_duration_since(p.sent_at) >= ACK_TIMEOUT)
            .map(|(id, _)| id.clone())
            .collect();
        let mut out = Vec::with_capacity(late.len());
        for cmd_id in late {
            let Some(mut p) = self.pending.remove(&cmd_id) else {
                continue;
            };
            match p.resend.clone() {
                Some(frame) if !p.retried => {
                    p.retried = true;
                    p.sent_at = now;
                    out.push(Overdue::Resend {
                        cmd_id: cmd_id.clone(),
                        what: p.what.clone(),
                        frame,
                    });
                    self.pending.insert(cmd_id, p);
                }
                _ => out.push(Overdue::Lost {
                    cmd_id,
                    what: p.what,
                }),
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(id: &str) -> ServerMsg {
        ServerMsg::Write {
            cmd_id: id.into(),
            gpio: 4,
            value: json!(1),
        }
    }

    fn command(id: &str) -> ServerMsg {
        ServerMsg::Command {
            cmd_id: id.into(),
            name: "power".into(),
            args: json!({"value": 0}),
        }
    }

    #[test]
    fn acknowledged_commands_are_forgotten() {
        let t0 = Instant::now();
        let mut acks = PendingAcks::default();
        acks.track(&write("a"), "frame-a", t0);
        let (what, rtt) = acks.ack("a", t0 + Duration::from_millis(180)).unwrap();
        assert_eq!(what, "write gpio 4");
        assert_eq!(rtt, Duration::from_millis(180));
        assert!(acks.is_empty());
        assert!(acks.overdue(t0 + ACK_TIMEOUT * 2).is_empty());
        assert!(acks.ack("unknown", t0).is_none());
    }

    #[test]
    fn a_lost_write_is_resent_once_then_reported() {
        let t0 = Instant::now();
        let mut acks = PendingAcks::default();
        acks.track(&write("a"), "frame-a", t0);
        assert!(acks.overdue(t0 + Duration::from_secs(1)).is_empty());
        let t1 = t0 + ACK_TIMEOUT;
        assert_eq!(
            acks.overdue(t1),
            vec![Overdue::Resend {
                cmd_id: "a".into(),
                what: "write gpio 4".into(),
                frame: "frame-a".into()
            }]
        );
        assert_eq!(
            acks.overdue(t1 + ACK_TIMEOUT),
            vec![Overdue::Lost {
                cmd_id: "a".into(),
                what: "write gpio 4".into()
            }]
        );
        assert!(acks.is_empty());
    }

    #[test]
    fn a_lost_custom_command_is_never_resent() {
        let t0 = Instant::now();
        let mut acks = PendingAcks::default();
        acks.track(&command("c"), "frame-c", t0);
        assert_eq!(
            acks.overdue(t0 + ACK_TIMEOUT),
            vec![Overdue::Lost {
                cmd_id: "c".into(),
                what: "command power".into()
            }]
        );
    }

    #[test]
    fn other_messages_are_not_tracked_and_the_map_is_bounded() {
        let t0 = Instant::now();
        let mut acks = PendingAcks::default();
        acks.track(
            &ServerMsg::Subscribe {
                cmd_id: "s".into(),
                gpio: 1,
                interval_ms: 1000,
            },
            "f",
            t0,
        );
        assert!(acks.is_empty());
        for i in 0..(MAX_PENDING + 10) {
            acks.track(
                &write(&i.to_string()),
                "f",
                t0 + Duration::from_millis(i as u64),
            );
        }
        assert_eq!(acks.pending.len(), MAX_PENDING);
        assert!(!acks.pending.contains_key("0"), "oldest dropped first");
    }
}
