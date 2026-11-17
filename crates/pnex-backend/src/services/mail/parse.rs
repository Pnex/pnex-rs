//! From raw bytes to a stored message (mail.md D458, D463): headers read,
//! text kept (the HTML part only as text), attachments listed but never
//! opened, automatic replies and our own mails flagged, the original of a
//! forward read from an attached message or from the forward block. Each
//! attached message also comes out as a message of its own (D994).

use mail_parser::{Address, Message, MessageParser, MimeHeaders};
use pnex_core::mail::{
    clean_text, html_to_text, is_automatic, is_from_pnex, parse_forward_chain, MailAddress,
    MailAttachment, MailAuth, MailForward, RECIPIENTS_MAX, SUBJECT_MAX, TEXT_MAX,
};

/// Largest `text/calendar` part read (D466).
const CALENDAR_MAX: usize = 256 * 1024;

/// Attached messages split out of one message at most (D994).
pub const ATTACHED_MAX: usize = 30;

/// What the parser read from one message; the poller adds the mailbox,
/// folder, ids and trust.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub message_id: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub from: MailAddress,
    pub to: Vec<MailAddress>,
    pub cc: Vec<MailAddress>,
    pub subject: String,
    pub date: Option<String>,
    pub text: String,
    pub truncated: bool,
    pub attachments: Vec<MailAttachment>,
    pub auth: MailAuth,
    pub automatic: bool,
    pub from_pnex: bool,
    /// The hops of a forward, the latest first, not yet trusted (the
    /// poller picks the original, D463).
    pub forwards: Vec<MailForward>,
    /// The meeting invitation of a `text/calendar` part (D466).
    pub invitation: Option<pnex_core::mail::MailInvitation>,
    /// The messages forwarded « as attachment » (`.eml`), each one a
    /// forward of its own from the same sender (D994); empty on them.
    pub attached: Vec<Parsed>,
}

fn addresses(a: Option<&Address<'_>>) -> Vec<MailAddress> {
    let Some(a) = a else {
        return Vec::new();
    };
    a.iter()
        .filter_map(|x| {
            Some(MailAddress {
                name: x.name().unwrap_or_default().chars().take(200).collect(),
                address: x.address()?.trim().to_ascii_lowercase(),
            })
        })
        .filter(|x| x.address.contains('@') && x.address.len() <= pnex_core::mail::ADDRESS_MAX)
        .take(RECIPIENTS_MAX)
        .collect()
}

/// Bounded single-line header text.
fn line(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Parses one message; `own` = the org's own addresses (D457.2);
/// `fallback_id` names a message without a `Message-ID`.
pub fn parse(raw: &[u8], truncated_raw: bool, own: &[String], fallback_id: &str) -> Option<Parsed> {
    let msg = MessageParser::default().parse(raw)?;
    let headers: Vec<(String, String)> = msg
        .headers_raw()
        .map(|(k, v)| (k.to_string(), line(v, 2000)))
        .collect();
    let from = addresses(msg.from()).into_iter().next().unwrap_or_default();
    let content_type = msg
        .content_type()
        .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or_default()))
        .unwrap_or_default();
    let subject = line(msg.subject().unwrap_or_default(), SUBJECT_MAX);
    let (text, cut) = clean_text(&body(&msg), TEXT_MAX);
    // Never read from the message (the sender may write it): the poller
    // verifies the signature itself (`verify`).
    let auth = MailAuth::default();
    let mut attachments = Vec::new();
    // A forward « as attachment »: its own headers are the original.
    let mut originals = Vec::new();
    for part in msg.attachments() {
        let ct = part
            .content_type()
            .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or_default()))
            .unwrap_or_default();
        if let Some(inner) = part.message().filter(|_| originals.len() < ATTACHED_MAX) {
            if let Some(f) = addresses(inner.from()).into_iter().next() {
                let forward = MailForward {
                    from: f,
                    date: inner.date().map(|d| d.to_rfc3339()),
                    subject: line(inner.subject().unwrap_or_default(), SUBJECT_MAX),
                    trusted: false,
                    member: false,
                };
                originals.push((forward, clean_text(&body(inner), TEXT_MAX)));
            }
        }
        attachments.push(MailAttachment {
            name: line(part.attachment_name().unwrap_or("attachment"), 255),
            content_type: line(&ct, 255),
            size: part.len() as u64,
            media_id: None,
        });
        if attachments.len() >= 100 {
            break;
        }
    }
    let message_id = msg
        .message_id()
        .map(|m| line(m, 990))
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| fallback_id.to_string());
    // An invitation is a `text/calendar` part, inline or attached; only
    // read as text by our own iCalendar reader, never run.
    let invitation = msg
        .parts
        .iter()
        .filter(|p| {
            p.content_type().is_some_and(|c| {
                c.ctype().eq_ignore_ascii_case("text")
                    && c.subtype()
                        .is_some_and(|s| s.eq_ignore_ascii_case("calendar"))
            })
        })
        .filter(|p| p.len() <= CALENDAR_MAX)
        .find_map(|p| {
            let text = p
                .text_contents()
                .map(str::to_string)
                .or_else(|| std::str::from_utf8(p.contents()).ok().map(str::to_string))?;
            crate::services::time_ranges::import::invitation(&text)
        });
    let in_reply_to = msg
        .in_reply_to()
        .as_text()
        .map(|m| line(m, 990))
        .filter(|m| !m.is_empty());
    let references = msg
        .references()
        .as_text_list()
        .map(|l| l.iter().take(50).map(|r| line(r, 990)).collect())
        .unwrap_or_default();
    let date = msg.date().map(|d| d.to_rfc3339());
    let automatic = is_automatic(&headers, &from.address, &content_type);
    let from_pnex = is_from_pnex(&headers, &from.address, own);
    let to = addresses(msg.to());
    let cc = addresses(msg.cc());
    // Each attached message is a forward of its own, sent by the same
    // person (the poller gives it the carrier's authentication: the
    // signature covers the attachment). Its id is derived from the
    // carrier's, so a second poll stores it once.
    let attached: Vec<Parsed> = originals
        .into_iter()
        .enumerate()
        .map(|(i, (forward, (inner_text, inner_cut)))| Parsed {
            message_id: format!("{message_id}#{i}"),
            in_reply_to: None,
            references: vec![message_id.clone()],
            from: from.clone(),
            to: to.clone(),
            cc: cc.clone(),
            subject: forward.subject.clone(),
            date: date.clone(),
            text: inner_text,
            truncated: inner_cut || truncated_raw,
            // shortcut: an attachment index points into the carrier, so the
            // inner message lists none; list them when someone needs to save one.
            attachments: Vec::new(),
            auth,
            automatic,
            from_pnex,
            forwards: vec![forward],
            invitation: None,
            attached: Vec::new(),
        })
        .collect();
    // The carrier of attached messages is no forward itself: its text is
    // the sender's own note.
    let forwards = if attached.is_empty() {
        parse_forward_chain(&subject, &text)
    } else {
        Vec::new()
    };
    Some(Parsed {
        invitation,
        automatic,
        from_pnex,
        message_id,
        in_reply_to,
        references,
        to,
        cc,
        from,
        subject,
        date,
        text,
        truncated: cut || truncated_raw,
        attachments,
        auth,
        forwards,
        attached,
    })
}

/// The text of a message: a real text/plain part, else our own conversion
/// of the HTML part (the parser's would keep what we drop: scripts, styles).
fn body(msg: &Message<'_>) -> String {
    let plain = msg
        .text_bodies()
        .find(|p| {
            p.content_type().is_none_or(|c| {
                c.ctype().eq_ignore_ascii_case("text")
                    && c.subtype().is_none_or(|s| s.eq_ignore_ascii_case("plain"))
            })
        })
        .and_then(|p| p.text_contents())
        .filter(|t| !t.trim().is_empty());
    match (plain, msg.body_html(0)) {
        (Some(t), _) => t.to_string(),
        (None, Some(h)) => html_to_text(&h),
        (None, None) => String::new(),
    }
}

/// One attachment taken out of a message (mail.md M5), still unopened:
/// the caller passes its bytes to the media allowlist.
#[derive(Debug)]
pub struct Extracted {
    /// The `Message-ID` read again, to check the server still holds the
    /// stored message under this UID.
    pub message_id: Option<String>,
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// The attachment at `index`, in the order [`parse`] lists them.
pub fn attachment(raw: &[u8], index: usize) -> Option<Extracted> {
    let msg = MessageParser::default().parse(raw)?;
    let part = msg.attachments().take(100).nth(index)?;
    Some(Extracted {
        message_id: msg
            .message_id()
            .map(|m| line(m, 990))
            .filter(|m| !m.is_empty()),
        name: line(part.attachment_name().unwrap_or("attachment"), 255),
        content_type: part
            .content_type()
            .map(|c| {
                line(
                    &format!("{}/{}", c.ctype(), c.subtype().unwrap_or_default()),
                    255,
                )
            })
            .unwrap_or_default(),
        bytes: part.contents().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEWSLETTER: &str = "From: Veille Industrie <news@veille.fr>\r\n\
To: test@pnex.io\r\n\
Subject: =?UTF-8?Q?Usine_=C3=A0_Lyon?=\r\n\
Date: Mon, 26 Oct 2026 08:00:00 +0100\r\n\
Message-ID: <n1@veille.fr>\r\n\
List-Unsubscribe: <mailto:unsub@veille.fr>\r\n\
Authentication-Results: mx.o2switch.net; spf=pass smtp.mailfrom=veille.fr; dkim=pass; dmarc=pass\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/alternative; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>Nouvelle usine&nbsp;!</p><script>steal()</script><img src=\"https://tracker/x.gif\">\r\n\
--b--\r\n";

    #[test]
    fn a_newsletter_is_text_flagged_automatic_and_never_believed() {
        let p = parse(NEWSLETTER.as_bytes(), false, &[], "fallback").unwrap();
        assert_eq!(p.subject, "Usine à Lyon");
        assert_eq!(p.from.address, "news@veille.fr");
        assert_eq!(p.message_id, "n1@veille.fr");
        assert!(p.text.contains("Nouvelle usine !"), "{}", p.text);
        assert!(
            !p.text.contains("steal") && !p.text.contains("tracker"),
            "{}",
            p.text
        );
        assert!(p.automatic, "a list is automatic");
        assert!(
            !p.auth.trusted(),
            "its own Authentication-Results is ignored"
        );
        assert!(p.date.unwrap().starts_with("2026-10-26"));
    }

    #[test]
    fn a_forward_as_attachment_names_the_original_sender() {
        let raw = "From: Bob <bob@example.com>\r\n\
To: veille@pnex.io\r\n\
Subject: Fwd: Devis\r\n\
Message-ID: <f1@example.com>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"m\"\r\n\
\r\n\
--m\r\n\
Content-Type: text/plain\r\n\
\r\n\
Pour le dossier Acme.\r\n\
--m\r\n\
Content-Type: message/rfc822\r\n\
Content-Disposition: attachment; filename=\"devis.eml\"\r\n\
\r\n\
From: =?UTF-8?Q?=C3=89lodie?= <elodie@acme.fr>\r\n\
Subject: Devis ligne 3\r\n\
Date: Fri, 23 Oct 2026 10:00:00 +0200\r\n\
\r\n\
Bonjour, voici notre besoin.\r\n\
--m--\r\n";
        let p = parse(raw.as_bytes(), false, &[], "x").unwrap();
        assert!(p.forwards.is_empty(), "the carrier is the sender's note");
        assert_eq!(p.attached.len(), 1);
        let inner = &p.attached[0];
        assert_eq!(inner.message_id, "f1@example.com#0");
        assert_eq!(inner.from.address, "bob@example.com");
        assert_eq!(inner.subject, "Devis ligne 3");
        assert!(inner.text.contains("voici notre besoin"), "{}", inner.text);
        let f = inner.forwards.first().expect("forward read");
        assert_eq!(
            (f.from.address.as_str(), f.from.name.as_str()),
            ("elodie@acme.fr", "Élodie")
        );
        assert_eq!(f.subject, "Devis ligne 3");
        assert!(!f.trusted);
        assert_eq!(p.attachments.len(), 1);
        assert!(!p.automatic);
        let a = attachment(raw.as_bytes(), 0).expect("attachment 0");
        assert_eq!(a.message_id.as_deref(), Some("f1@example.com"));
        assert_eq!(a.name, "devis.eml");
        assert!(attachment(raw.as_bytes(), 1).is_none());
    }

    #[test]
    fn our_own_mail_and_a_missing_id_are_handled() {
        let raw = "From: sales@pnex.io\r\nSubject: hello\r\nX-PneX-Origin: 1/m/2\r\n\r\nhi\r\n";
        let p = parse(
            raw.as_bytes(),
            true,
            &["sales@pnex.io".into()],
            "<7.42@mailbox>",
        )
        .unwrap();
        assert!(p.from_pnex);
        assert_eq!(p.message_id, "<7.42@mailbox>");
        assert!(p.truncated);
    }

    #[test]
    fn several_newsletters_attached_become_one_forward_each() {
        let eml = |n: u32| {
            format!(
                "--m\r\nContent-Type: message/rfc822\r\n\r\n\
From: Lettre {n} <news{n}@veille.fr>\r\nSubject: Lettre {n}\r\n\
List-Unsubscribe: <mailto:u@veille.fr>\r\nContent-Type: text/html\r\n\r\n\
<p>Usine {n} ouverte</p>\r\n"
            )
        };
        let raw = format!(
            "From: dir@acme.fr\r\nSubject: Articles\r\nMessage-ID: <c1@acme.fr>\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"m\"\r\n\r\n\
--m\r\nContent-Type: text/plain\r\n\r\nSignature only\r\n{}{}--m--\r\n",
            eml(1),
            eml(2)
        );
        let p = parse(raw.as_bytes(), false, &[], "x").unwrap();
        let got: Vec<_> = p
            .attached
            .iter()
            .map(|a| {
                (
                    a.message_id.as_str(),
                    a.forwards[0].from.address.as_str(),
                    a.automatic,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("c1@acme.fr#0", "news1@veille.fr", false),
                ("c1@acme.fr#1", "news2@veille.fr", false)
            ]
        );
        assert!(p.attached[1].text.contains("Usine 2 ouverte"));
        assert_eq!(p.attachments.len(), 2);
    }

    #[test]
    fn an_attachment_comes_out_decoded() {
        let raw = "From: elodie@acme.fr\r\nSubject: Facture\r\nMessage-ID: <inv1@acme.fr>\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"m\"\r\n\r\n\
--m\r\nContent-Type: text/plain\r\n\r\nCi-joint.\r\n\
--m\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"facture.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--m--\r\n";
        let a = attachment(raw.as_bytes(), 0).unwrap();
        assert_eq!(
            (a.name.as_str(), a.content_type.as_str(), a.bytes.as_slice()),
            ("facture.pdf", "application/pdf", b"%PDF-1.4\n".as_slice())
        );
        assert_eq!(a.message_id.as_deref(), Some("inv1@acme.fr"));
    }
}
