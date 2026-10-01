//! Pont caméra JS — getUserMedia dans la webview (module cfg Android).
//!
//! wry gère déjà le flux runtime-permission (RustWebChromeClient.kt mappe
//! `VIDEO_CAPTURE` → `CAMERA` avec launcher Capacitor) et le contenu est
//! servi via WebViewAssetLoader (contexte sécurisé) : côté Rust, il suffit
//! d'injecter le JS (école `media_viewer.rs`, `document::eval`).
//!
//! Séquence par frame : canvas réduit à ~1000 px → `toDataURL('image/jpeg')`
//! → stash global → récupération par chunks de 128 KiB (on ne traverse pas
//! le pont IPC avec de gros payloads non testés) → décodage base64 (crate
//! `base64`, déjà dep).

use base64::Engine as _;

/// Plafond de largeur de frame (V2) : le canvas capture la piste pleine
/// résolution jusqu'à 1080 de large (au-delà, downscale) — la hfov corrigée
/// (43,7°) donne ~16 px/° à 720 ; 1080 porterait à ~25 px/°.
const FRAME_MAX_WIDTH: u32 = 1080;
/// Qualité JPEG des frames.
const FRAME_QUALITY: f64 = 0.8;
/// Taille des chunks base64 traversant le pont IPC.
const CHUNK: usize = 128 * 1024;

/// Erreur du pont caméra.
#[derive(Debug)]
pub enum FrameError {
    /// Permission refusée (NotAllowedError) ou pré-check perdu.
    Permission,
    /// Pas de caméra (NotFoundError / OverconstrainedError / track absent).
    NoVideo,
    /// Autre échec JS (détail pour le log).
    Unknown(String),
}

/// Ouvre la caméra arrière et l'attache au `<video>` (srcObject).
/// Retourne (largeur, hauteur) du track (0 si inconnues).
///
/// ⚠️ PAS d'async dans l'eval : côté natif, `document::eval` ne await pas
/// les promesses (pattern prouvé tron.rs/media_viewer.rs : return sync) —
/// on lance getUserMedia en fire-and-forget (.then/.catch posent des
/// globales) puis on sonde un eval SYNCHRONE.
pub async fn open_camera(video_id: &str) -> Result<(u32, u32), FrameError> {
    let id_q = video_id.replace('\'', "");
    // 1) Lancement fire-and-forget — le résultat atterrit dans
    //    window.__p360Ready / __p360Err.
    let _ = dioxus::document::eval(&format!(
        "window.__p360Ready = false; \
         window.__p360Err = ''; \
         navigator.mediaDevices.getUserMedia({{ audio: false, \
             video: {{ facingMode: {{ ideal: 'environment' }}, \
                 width: {{ ideal: 1920 }}, height: {{ ideal: 1080 }} }} }}) \
         .then(s => {{ \
             window.__p360Stream = s; \
             const v = document.getElementById('{id_q}'); \
             if (v) {{ v.srcObject = s; v.muted = true; v.play(); }} \
             window.__p360Ready = true; \
         }}) \
         .catch(e => {{ window.__p360Err = (e && e.name) || 'Error'; }});",
        id_q = id_q
    ));

    // 2) Sondage synchrone (~6 s max).
    for _ in 0..40 {
        crate::util::sleep(std::time::Duration::from_millis(150)).await;
        let value = eval_value(&format!(
            "const s = window.__p360Stream; \
             const v = document.getElementById('{id_q}'); \
             if (s && v && v.srcObject !== s) {{ v.srcObject = s; v.muted = true; v.play(); }} \
             return JSON.stringify({{ ready: !!window.__p360Ready, \
                 err: window.__p360Err || '', \
                 w: s ? (s.getVideoTracks()[0]?.getSettings().width || 0) : 0, \
                 h: s ? (s.getVideoTracks()[0]?.getSettings().height || 0) : 0 }})",
        ))
        .await?;
        if value.get("err").and_then(|v| v.as_str()).unwrap_or("") != "" {
            let err = value.get("err").and_then(|v| v.as_str()).unwrap_or("");
            return match err {
                "NotAllowedError" => Err(FrameError::Permission),
                "NotFoundError" | "OverconstrainedError" => Err(FrameError::NoVideo),
                _ => Err(FrameError::Unknown(err.to_string())),
            };
        }
        if value
            .get("ready")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let w = value.get("w").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let h = value.get("h").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            return Ok((w, h));
        }
    }
    Err(FrameError::Unknown("getUserMedia : timeout".into()))
}

/// Capture la frame courante du `<video>` : canvas réduit + JPEG + base64
/// récupéré par chunks.
pub async fn grab_frame(video_id: &str) -> Result<Vec<u8>, FrameError> {
    let id_q = video_id.replace('\'', "");
    // 1) Rendu + stash, retour des métadonnées de découpage.
    let meta = eval_value(&format!(
        "const v = document.getElementById('{id_q}'); \
         if (!v || !v.videoWidth) return JSON.stringify({{ ok: false, err: 'NoVideo' }}); \
         const tw = Math.min({frame_w}, v.videoWidth); \
         const c = document.createElement('canvas'); \
         c.width = tw; c.height = Math.round(v.videoHeight * tw / v.videoWidth); \
         c.getContext('2d').drawImage(v, 0, 0, c.width, c.height); \
         window.__p360Frame = c.toDataURL('image/jpeg', {quality}); \
         return JSON.stringify({{ ok: true, \
             len: window.__p360Frame.length, \
             chunks: Math.ceil(window.__p360Frame.length / {chunk}) }});",
        id_q = id_q,
        // Pass-through jusqu'à 1080 de large : la piste getUserMedia peut
        // livrer plus que le plafond historique 720 (les devices récents
        // offrent 1080×1920 portrait) — pas de downscale inutile.
        frame_w = FRAME_MAX_WIDTH,
        quality = FRAME_QUALITY,
        chunk = CHUNK
    ))
    .await?;
    if !meta.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Err(FrameError::NoVideo);
    }
    let chunks = meta.get("chunks").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    if chunks == 0 {
        return Err(FrameError::Unknown("payload vide".into()));
    }

    // 2) Récupération chunk par chunk.
    let mut payload = String::with_capacity(chunks * CHUNK);
    for i in 0..chunks {
        // ⚠️ chunk = base64 BRUT — pas du JSON : eval_string, pas eval_value.
        let piece = eval_string(&format!(
            "return window.__p360Frame.slice({start}, {end})",
            start = i * CHUNK,
            end = (i + 1) * CHUNK
        ))
        .await?;
        payload.push_str(&piece);
    }

    // 3) Libère le stash global.
    let _ = dioxus::document::eval("window.__p360Frame = null").await;

    // 4) Strip du préfixe data: + décodage base64.
    let Some(b64) = payload.strip_prefix("data:image/jpeg;base64,") else {
        return Err(FrameError::Unknown("missing data: prefix".into()));
    };
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| FrameError::Unknown(format!("base64 : {e}")))
}

/// Arrête le flux caméra et déstash — fire-and-forget.
pub fn close_camera() {
    dioxus::prelude::spawn(async move {
        let _ = dioxus::document::eval(
            "try { window.__p360Stream && window.__p360Stream.getTracks().forEach(t => t.stop()); } catch (e) {} window.__p360Stream = null; window.__p360Frame = null; window.__p360Ready = false; window.__p360Err = '';",

        )
        .await;
    });
}

/// eval JS → String brute (PAS de parse JSON — pour les payloads base64).
async fn eval_string(js: &str) -> Result<String, FrameError> {
    let value = dioxus::document::eval(js)
        .await
        .map_err(|e| FrameError::Unknown(format!("eval : {e:?}")))?;
    if value.is_string() {
        Ok(value.as_str().unwrap_or("").to_string())
    } else {
        serde_json::to_string(&value).map_err(|e| FrameError::Unknown(format!("json : {e}")))
    }
}

/// eval JS → serde_json::Value (le JS doit retourner une string JSON ou
/// une valeur JSON-able ; échec → FrameError::Unknown).
async fn eval_value(js: &str) -> Result<serde_json::Value, FrameError> {
    let value = dioxus::document::eval(js)
        .await
        .map_err(|e| FrameError::Unknown(format!("eval : {e:?}")))?;
    // Le pont dioxus peut livrer une string JSON ou un objet déjà parsé.
    if value.is_string() {
        let s = value.as_str().unwrap_or("");
        serde_json::from_str(s).map_err(|e| FrameError::Unknown(format!("json : {e}")))
    } else {
        serde_json::to_value(&value).map_err(|e| FrameError::Unknown(format!("json : {e}")))
    }
}
