//! Page `/streams/:id/review` — review of a window of a live stream
//! (media-review.md D271 V2): the text and the speakers of the window, the
//! audio still kept by the retention played piece by piece. Same panels as
//! the review of a media file; no attribution here (they live on a media).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::media_review::{piece_at, AudioPiece, MediaTranscript};

use crate::api;
use crate::app::Route;
use crate::components::crud::layout::ListLayout;
use crate::components::timeline::axis::Scale;
use crate::components::timeline::{TimeBase, Timeline};
use crate::components::transport::{play_media, seek_media, MediaPlayer};
use crate::components::ui::{BTN_SECONDARY_SM, PANEL};
use crate::pages::media::review::{lanes, speakers, TranscriptPanel, PLAYER};

const CANVAS: &str = "stream-review-frise";
const HOUR_MS: i64 = 3_600_000;

#[component]
pub fn StreamReview(id: String) -> Element {
    let navigator = use_navigator();
    // Window: its end (ms since epoch) and its length.
    let mut end_ms = use_signal(|| chrono::Utc::now().timestamp_millis());
    let mut span_ms = use_signal(|| HOUR_MS);
    let mut pos_ms = use_signal(|| 0i64);
    let mut piece = use_signal(|| None::<usize>);
    let mut blob = use_signal(|| None::<String>);
    // Seek waiting for the piece's audio to load (ms in the piece), and
    // whether to play once loaded (chained pieces).
    let mut pending = use_signal(|| None::<(i64, bool)>);
    let rid = id.clone();
    let review = use_resource(move || {
        let id = rid.clone();
        let to = end_ms();
        let from = to - span_ms();
        async move {
            let iso = |ms: i64| {
                chrono::DateTime::from_timestamp_millis(ms)
                    .unwrap_or_default()
                    .to_rfc3339()
            };
            api::media_streams::review(&id, &iso(from), &iso(to)).await
        }
    });
    let data = review
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .cloned();
    let failed = matches!(&*review.read(), Some(Err(_)));
    let pieces: Vec<AudioPiece> = data.as_ref().map(|r| r.audio.clone()).unwrap_or_default();
    let as_transcript = data.as_ref().map(|r| MediaTranscript {
        runs: Vec::new(),
        run: None,
        segments: r.segments.clone(),
        unavailable: r.unavailable,
        attributions: Vec::new(),
    });
    let all = as_transcript
        .as_ref()
        .map(|t| speakers(&t.segments))
        .unwrap_or_default();

    // Loads a piece and moves the cursor to `ms` (window clock).
    let sid = id.clone();
    let mut open_piece = move |k: usize, ms: i64, play: bool, pieces: &[AudioPiece]| {
        let Some(p) = pieces.get(k).cloned() else {
            return;
        };
        pos_ms.set(ms);
        if piece() == Some(k) {
            seek_media(PLAYER, (ms - p.start_ms) as f64 / 1000.0);
            return;
        }
        piece.set(Some(k));
        pending.set(Some((ms - p.start_ms, play)));
        let path = api::media_streams::segment_audio_path(&sid, &p.segment_id.to_string());
        spawn(async move {
            blob.set(None);
            blob.set(crate::util::media_blob_url(&path, Some("audio/wav")).await);
        });
    };
    let seek = {
        let pieces = pieces.clone();
        let mut open_piece = open_piece.clone();
        move |ms: i64| match piece_at(&pieces, ms) {
            Some(k) => open_piece(k, ms, false, &pieces),
            None => pos_ms.set(ms),
        }
    };
    let on_time = {
        let pieces = pieces.clone();
        move |(p, d): (f64, f64)| {
            let Some(k) = piece() else {
                return;
            };
            let Some(cur) = pieces.get(k) else {
                return;
            };
            if let Some((at, play)) = pending() {
                if d > 0.0 {
                    pending.set(None);
                    seek_media(PLAYER, at as f64 / 1000.0);
                    if play {
                        play_media(PLAYER);
                    }
                }
                return;
            }
            pos_ms.set(cur.start_ms + (p * 1000.0) as i64);
            // End of a piece: the next one follows.
            if d > 0.0 && p >= d - 0.25 && k + 1 < pieces.len() {
                let next = pieces[k + 1].start_ms.max(cur.end_ms);
                open_piece(k + 1, next, true, &pieces);
            }
        }
    };
    let lanes_now = as_transcript
        .as_ref()
        .map(|t| lanes(t, &all, None, false))
        .unwrap_or_default();
    let window = span_ms();
    let start_label = chrono::DateTime::from_timestamp_millis(end_ms() - window)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%d/%m %H:%M")
                .to_string()
        })
        .unwrap_or_default();

    rsx! {
        ListLayout {
            title: t!("streams-review-title").to_string(),
            subtitle: Some(
                t!("streams-review-window", from : start_label, hours : window / HOUR_MS)
                    .to_string(),
            ),
            on_back: move |_| {
                navigator.push(Route::Streams {});
            },
            can_write: false,
            div { class: "mb-3 flex flex-wrap items-center gap-2",
                // shortcut: presets and shifts only; the shared WindowPicker of
                // D220 does not exist yet, use it once it does.
                for h in [1i64, 6] {
                    button {
                        key: "{h}",
                        class: BTN_SECONDARY_SM,
                        r#type: "button",
                        onclick: move |_| {
                            span_ms.set(h * HOUR_MS);
                            piece.set(None);
                        },
                        {t!("streams-review-hours", n : h)}
                    }
                }
                button {
                    class: BTN_SECONDARY_SM,
                    r#type: "button",
                    onclick: move |_| {
                        end_ms -= span_ms();
                        piece.set(None);
                    },
                    {t!("streams-review-earlier")}
                }
                button {
                    class: BTN_SECONDARY_SM,
                    r#type: "button",
                    onclick: move |_| {
                        end_ms += span_ms();
                        piece.set(None);
                    },
                    {t!("streams-review-later")}
                }
                button {
                    class: BTN_SECONDARY_SM,
                    r#type: "button",
                    onclick: move |_| {
                        end_ms.set(chrono::Utc::now().timestamp_millis());
                        piece.set(None);
                    },
                    {t!("streams-review-now")}
                }
            }
            div { class: "grid gap-4 lg:grid-cols-5",
                div { class: "lg:col-span-3 space-y-3",
                    if pieces.is_empty() {
                        p { class: "rounded-md bg-gray-50 px-3 py-2 text-sm text-gray-600",
                            {t!("streams-review-no-audio")}
                        }
                    } else if let Some(url) = blob() {
                        MediaPlayer {
                            id: PLAYER,
                            src: url,
                            video: false,
                            on_time,
                        }
                    } else {
                        p { class: "text-sm text-gray-600", {t!("streams-review-pick")} }
                    }
                }
                TranscriptPanel {
                    asset_id: String::new(),
                    can_write: false,
                    data: as_transcript.clone(),
                    failed,
                    speakers: all.clone(),
                    attributions: Vec::new(),
                    pos: pos_ms(),
                    on_seek: seek.clone(),
                }
            }
            if !lanes_now.is_empty() {
                div { class: "mt-4 {PANEL}",
                    Timeline {
                        canvas: CANVAS,
                        scale: Scale::linear((0, window)),
                        base: TimeBase::Media,
                        cursor: pos_ms(),
                        lanes: lanes_now,
                        header: t!("media-review-speakers").to_string(),
                        on_seek: seek,
                    }
                }
            }
        }
    }
}
