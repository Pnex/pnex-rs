//! Live view of one camera (D75): `pnexViewers.camera` mounted into a host
//! div for as long as the component lives — closing the modal closes the
//! socket, so an `on_demand` camera may go back to sleep.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;

/// Period at which the live viewer receives the current WebSocket URL.
const TOKEN_REFRESH_SECS: u64 = 20;

/// Live view: host div + `pnexViewers.camera` mount after the first render,
/// unmount on drop (closes the socket — the server then lets an on-demand
/// camera sleep after its grace delay).
#[component]
pub(super) fn LiveView(device: i64) -> Element {
    let host_id = format!("pnex-camera-live-{device}");
    let mut failed = use_signal(|| false);
    let host_for_mount = host_id.clone();
    let host_for_drop = host_id.clone();

    use_effect(move || {
        let host = host_for_mount.clone();
        spawn(async move {
            let Some(url) = api::cameras::live_ws_url(device) else {
                failed.set(true);
                return;
            };
            if !crate::media_viewer::mount("camera", &host, &url).await {
                failed.set(true);
            }
        });
    });
    // Token freshness: the page re-fetches the camera list every few
    // seconds (the HTTP client refreshes an expired access token on 401);
    // push the current URL so a reconnect never replays a stale token.
    let host_for_refresh = host_id.clone();
    use_future(move || {
        let host = host_for_refresh.clone();
        async move {
            loop {
                crate::util::sleep(std::time::Duration::from_secs(TOKEN_REFRESH_SECS)).await;
                if let Some(url) = api::cameras::live_ws_url(device) {
                    crate::media_viewer::camera_set_url(&host, &url);
                }
            }
        }
    });
    use_drop(move || {
        crate::media_viewer::unmount(&host_for_drop);
    });

    rsx! {
        div { class: "relative aspect-video bg-gray-900 rounded-lg overflow-hidden",
            div { id: "{host_id}", class: "w-full h-full" }
            if failed() {
                div { class: "absolute inset-0 flex items-center justify-center text-sm text-gray-300 p-4 text-center",
                    {t!("cameras-live-unavailable")}
                }
            }
        }
    }
}
