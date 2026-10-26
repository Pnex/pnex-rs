//! The sender as PneX verifies it (mail.md D458): a DKIM signature that
//! verifies and whose signing domain is aligned with the From domain. The
//! `Authentication-Results` header of a message is never believed: a
//! receiving server that adds none (cPanel hosts) leaves the sender free to
//! write one. SPF is not evaluated: a mailbox never knows the connecting IP.

use std::sync::OnceLock;
use std::time::Duration;

use mail_auth::{AuthenticatedMessage, DkimResult, MessageAuthenticator};
use pnex_core::mail::{AuthVerdict, MailAuth};

/// DNS lookups of the signing keys, for one message.
const TIMEOUT: Duration = Duration::from_secs(15);

fn authenticator() -> Option<&'static MessageAuthenticator> {
    static A: OnceLock<Option<MessageAuthenticator>> = OnceLock::new();
    A.get_or_init(|| {
        MessageAuthenticator::new_system_conf()
            .map_err(|e| tracing::warn!(error = %e, "no DNS resolver: mails stay unverified"))
            .ok()
    })
    .as_ref()
}

/// Same domain, or one a subdomain of the other (relaxed alignment). Safe
/// without a public suffix list: a signature for `d=co.uk` needs a key
/// published in that zone.
pub fn aligned(signer: &str, from: &str) -> bool {
    let d = signer.trim_end_matches('.').to_ascii_lowercase();
    let f = from.trim_end_matches('.').to_ascii_lowercase();
    !d.is_empty()
        && !f.is_empty()
        && (d == f || f.ends_with(&format!(".{d}")) || d.ends_with(&format!(".{f}")))
}

/// The verdict from the signatures `(signing domain, passed, failed)`: an
/// aligned pass is a DKIM and DMARC pass; an aligned failure alone, a DKIM
/// failure; anything else (no signature, a third party's) says nothing.
pub fn verdict<'a>(from: &str, sigs: impl IntoIterator<Item = (&'a str, bool, bool)>) -> MailAuth {
    let mut dkim = AuthVerdict::None;
    for (d, pass, fail) in sigs {
        if !aligned(d, from) {
            continue;
        }
        if pass {
            return MailAuth {
                spf: AuthVerdict::None,
                dkim: AuthVerdict::Pass,
                dmarc: AuthVerdict::Pass,
            };
        }
        if fail {
            dkim = AuthVerdict::Fail;
        }
    }
    MailAuth {
        dkim,
        ..Default::default()
    }
}

/// Verifies a whole raw message (never a truncated one: its body hash
/// cannot match).
pub async fn auth(raw: &[u8]) -> MailAuth {
    let Some(msg) = AuthenticatedMessage::parse(raw) else {
        return MailAuth::default();
    };
    let from = msg
        .from()
        .rsplit_once('@')
        .map(|(_, d)| d.to_string())
        .unwrap_or_default();
    let Some(a) = authenticator() else {
        return MailAuth::default();
    };
    let Ok(out) = tokio::time::timeout(TIMEOUT, a.verify_dkim(&msg)).await else {
        tracing::info!("dkim verification timed out");
        return MailAuth::default();
    };
    verdict(
        &from,
        out.iter().map(|o| {
            (
                o.signature().map_or("", |s| s.d.as_str()),
                matches!(o.result(), DkimResult::Pass),
                matches!(o.result(), DkimResult::Fail(_)),
            )
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_is_the_domain_or_a_parent_or_a_child() {
        assert!(aligned("gmail.com", "gmail.com"));
        assert!(aligned("acme.fr", "ventes.acme.fr"));
        assert!(aligned("mail.acme.fr", "acme.fr"));
        assert!(aligned("ACME.fr.", "acme.fr"));
        assert!(!aligned("evil.com", "acme.fr"));
        assert!(!aligned("cme.fr", "acme.fr"), "a suffix is not a parent");
        assert!(!aligned("", "acme.fr"));
    }

    #[test]
    fn only_an_aligned_signature_counts() {
        let pass = verdict(
            "acme.fr",
            [("evil.com", false, true), ("acme.fr", true, false)],
        );
        assert!(pass.trusted());
        // A valid signature of the attacker's own domain on a forged From.
        let third = verdict("acme.fr", [("evil.com", true, false)]);
        assert_eq!(third, MailAuth::default());
        assert!(!third.trusted());
        let broken = verdict("acme.fr", [("acme.fr", false, true)]);
        assert_eq!(broken.dkim, AuthVerdict::Fail);
        assert!(!broken.trusted());
        assert!(!verdict("acme.fr", []).trusted());
    }
}
