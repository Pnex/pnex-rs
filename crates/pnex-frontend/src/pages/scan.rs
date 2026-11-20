//! Page `/scan` (parts-docs.md PD1, lot C-QR2): the rear camera reads a QR
//! code in the app, on the web and in the mobile app; a PNEX tag opens its
//! page `/q/<token>`. Where the browser cannot read codes, the phone's own
//! camera app opens the printed address just as well.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::app::Route;
use crate::components::crud::layout::ListLayout;

const READER: &str = r#"
const box = document.getElementById('pnex-scan');
if (!box) { return; }
if (!('BarcodeDetector' in window) || !navigator.mediaDevices) { dioxus.send('!unsupported'); return; }
let stream;
try {
  stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' }, audio: false });
} catch (e) { dioxus.send('!denied'); return; }
const v = document.createElement('video');
v.setAttribute('playsinline', '');
v.muted = true;
v.srcObject = stream;
v.style.width = '100%';
v.style.height = '100%';
v.style.objectFit = 'cover';
box.replaceChildren(v);
const stop = () => stream.getTracks().forEach((t) => t.stop());
window.__pnexScanStop = stop;
try { await v.play(); } catch (e) {}
const detector = new BarcodeDetector({ formats: ['qr_code'] });
while (document.body.contains(v)) {
  try {
    const codes = await detector.detect(v);
    if (codes.length) { stop(); dioxus.send(codes[0].rawValue); return; }
  } catch (e) {}
  await new Promise((r) => setTimeout(r, 250));
}
stop();
"#;

/// The token of a PNEX tag address (`…/q/<token>`), if the text is one.
pub fn token_of(text: &str) -> Option<String> {
    let after = text.trim().rsplit_once("/q/")?.1;
    let token = after.split(['?', '#', '/']).next()?;
    pnex_core::qr::valid_token(token).then(|| token.to_string())
}

#[component]
pub fn Scan() -> Element {
    let nav = use_navigator();
    // `None` while reading; otherwise the message key to show.
    let mut status = use_signal(|| None::<&'static str>);
    let mut round = use_signal(|| 0u32);
    use_effect(move || {
        let _ = round();
        status.set(None);
        spawn(async move {
            let mut ev = document::eval(READER);
            if let Ok(text) = ev.recv::<String>().await {
                match text.as_str() {
                    "!unsupported" => status.set(Some("scan-unsupported")),
                    "!denied" => status.set(Some("scan-denied")),
                    other => match token_of(other) {
                        Some(token) => {
                            nav.push(Route::QrScan { token });
                        }
                        None => status.set(Some("scan-not-pnex")),
                    },
                }
            }
        });
    });
    use_drop(|| {
        let _ = document::eval("window.__pnexScanStop && window.__pnexScanStop();");
    });
    rsx! {
        ListLayout {
            title: t!("scan-title").to_string(),
            subtitle: Some(t!("scan-subtitle").to_string()),
            can_write: false,
            div { class: "max-w-md space-y-3",
                div {
                    id: "pnex-scan",
                    class: "aspect-square w-full overflow-hidden rounded-md border border-line bg-ink",
                    "data-testid": "scan-view",
                }
                if let Some(key) = status() {
                    p {
                        class: "text-sm text-alarm",
                        "data-testid": "scan-status",
                        {t!(key)}
                    }
                    button {
                        r#type: "button",
                        class: crate::components::ui::BTN_SECONDARY,
                        onclick: move |_| round.with_mut(|r| *r += 1),
                        {t!("scan-again")}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pnex_tag_addresses_give_a_token() {
        let t = "a".repeat(26);
        assert_eq!(
            token_of(&format!("https://pnex.test/q/{t}")),
            Some(t.clone())
        );
        assert_eq!(
            token_of(&format!("https://pnex.test/q/{t}?x=1")),
            Some(t.clone())
        );
        assert_eq!(token_of("https://example.org/q/short"), None);
        assert_eq!(token_of("WIFI:S:home;;"), None);
    }
}
