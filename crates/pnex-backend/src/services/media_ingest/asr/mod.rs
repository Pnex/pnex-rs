//! Speech-to-text on the server side (media-ingest.md D166–D167): audio
//! models of the registry are extracted once to a local cache, then served
//! by warm `pnex-asr` processes ([`process`]).
//!
//! Settings (environment, platform level):
//! - `PNEX_ASR_BIN` (`pnex-asr`): the runtime build chosen for the host
//!   (sherpa CPU, whisper.cpp Vulkan…);
//! - `PNEX_ASR_MODELS_DIR` (temp dir): extracted models, rebuilt on demand;
//! - `PNEX_ASR_THREADS` (4), `PNEX_ASR_PROVIDER` (`cpu` | `cuda`);
//! - `PNEX_ASR_MAX_MEMORY_MB` (8192, 0 = none): address-space cap of a
//!   runtime process. Measured: Parakeet TDT 0.6B int8 hangs silently at
//!   1 GiB and loads from 2 GiB;
//! - `PNEX_ASR_SANDBOX` (`kernel` | `none`): GPU providers open device
//!   files read-write, which the kernel confinement refuses.

pub mod archive;
pub mod process;

use std::path::PathBuf;
use std::sync::OnceLock;

use loco_rs::app::AppContext;
use pnex_asr::protocol::{self, Family, Inspection};
use pnex_asr::wer::WerAccumulator;
use tokio::sync::Mutex;

use self::process::{AsrProcess, Launch};
use crate::models::_entities::ml_models;
use crate::services::media::MediaSettings;
use crate::services::media_ingest::capture::decoder;
use crate::services::media_ingest::capture::sandbox::SandboxMode;

/// `ml_models.task` of speech-to-text models.
pub const ASR_TASK: &str = "asr";

#[derive(Debug, Clone)]
pub struct AsrSettings {
    pub bin: String,
    pub models_dir: PathBuf,
    pub threads: u32,
    pub provider: String,
    pub sandbox: SandboxMode,
    /// Bytes, 0 = none.
    pub max_memory: u64,
}

impl AsrSettings {
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Self {
            bin: var("PNEX_ASR_BIN").unwrap_or_else(|| "pnex-asr".into()),
            models_dir: var("PNEX_ASR_MODELS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::temp_dir().join("pnex-asr-models")),
            threads: var("PNEX_ASR_THREADS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(4)
                .clamp(1, 64),
            provider: var("PNEX_ASR_PROVIDER").unwrap_or_else(|| "cpu".into()),
            max_memory: var("PNEX_ASR_MAX_MEMORY_MB")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(8192)
                * 1024
                * 1024,
            sandbox: match var("PNEX_ASR_SANDBOX").as_deref() {
                Some("none") => SandboxMode::None,
                _ => SandboxMode::Kernel,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AsrError {
    #[error("model file missing")]
    MissingBytes,
    #[error("model archive refused: {0}")]
    Archive(String),
    #[error("not a supported audio model: {0}")]
    Unsupported(String),
    #[error(transparent)]
    Process(#[from] process::AsrProcessError),
    #[error("internal: {0}")]
    Internal(String),
}

/// A model ready to serve: its directory and what it holds.
#[derive(Debug, Clone)]
pub struct LocalModel {
    pub dir: PathBuf,
    pub inspection: Inspection,
}

fn extraction_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Extracts the model's file (pinned version, else current) to the cache,
/// once per content hash, and inspects it.
pub async fn local_model(
    ctx: &AppContext,
    settings: &AsrSettings,
    asset_id: uuid::Uuid,
    asset_version: Option<i64>,
) -> Result<LocalModel, AsrError> {
    let version = crate::services::vision::asset_version_row(&ctx.db, asset_id, asset_version)
        .await
        .map_err(|e| AsrError::Internal(e.to_string()))?
        .ok_or(AsrError::MissingBytes)?;
    let tag = version
        .sha256
        .as_deref()
        .map(|s| s.chars().take(32).collect::<String>())
        .unwrap_or_else(|| version.id.simple().to_string());
    let dir = settings.models_dir.join(tag);
    let ready = dir.join(".ready");
    let _guard = extraction_lock().lock().await;
    if !ready.exists() {
        let store = MediaSettings::from_config(&ctx.config)
            .store()
            .map_err(AsrError::Internal)?;
        let bytes = store
            .get(&version.storage_key)
            .await
            .map_err(|_| AsrError::MissingBytes)?;
        let target = dir.clone();
        tokio::task::spawn_blocking(move || -> Result<(), AsrError> {
            let tmp = target.with_extension("partial");
            let _ = std::fs::remove_dir_all(&tmp);
            std::fs::create_dir_all(&tmp).map_err(|e| AsrError::Internal(e.to_string()))?;
            if let Err(e) = archive::extract(&bytes, &tmp) {
                let _ = std::fs::remove_dir_all(&tmp);
                return Err(AsrError::Archive(e.to_string()));
            }
            let _ = std::fs::remove_dir_all(&target);
            std::fs::rename(&tmp, &target).map_err(|e| AsrError::Internal(e.to_string()))?;
            std::fs::write(target.join(".ready"), b"")
                .map_err(|e| AsrError::Internal(e.to_string()))
        })
        .await
        .map_err(|e| AsrError::Internal(e.to_string()))??;
    }
    let inspection = protocol::inspect(&dir).map_err(AsrError::Unsupported)?;
    Ok(LocalModel { dir, inspection })
}

/// How to launch `model` for a language.
pub fn launch(
    settings: &AsrSettings,
    model: &LocalModel,
    family: Family,
    language: &str,
) -> Result<Launch, AsrError> {
    let bin = decoder::resolve(&settings.bin).ok_or(process::AsrProcessError::Missing)?;
    Ok(Launch {
        bin,
        dir: model.dir.clone(),
        family: family.wire().to_string(),
        language: if language == "auto" {
            "fr".into()
        } else {
            language.to_string()
        },
        threads: settings.threads,
        provider: settings.provider.clone(),
        max_memory: settings.max_memory,
    })
}

/// Result of the model check (D167): measured on this carrier.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckReport {
    pub family: String,
    pub runtime: String,
    pub engine: String,
    pub load_ms: u64,
    pub infer_ms: u64,
    /// Inference time / audio time.
    pub rtf: f64,
    /// Word error rate on the reference sample.
    pub wer: f64,
    pub transcript: String,
}

/// Loads the model in a fresh process (load time measured) and
/// transcribes the reference sample.
pub async fn check(ctx: &AppContext, model: &ml_models::Model) -> Result<CheckReport, AsrError> {
    let settings = AsrSettings::from_env();
    let local = local_model(ctx, &settings, model.asset_id, model.asset_version).await?;
    let family = local.inspection.family;
    let launch = launch(&settings, &local, family, "fr")?;
    let mut proc = AsrProcess::start(&launch, &settings.sandbox).await?;
    let resp = proc.transcribe(protocol::CHECK_WAV_FR).await?;
    let text = resp.text.unwrap_or_default();
    let mut wer = WerAccumulator::default();
    wer.add(protocol::CHECK_TEXT_FR, &text);
    let audio_ms = ((protocol::CHECK_WAV_FR.len().saturating_sub(44)) / 32) as f64;
    Ok(CheckReport {
        family: family.wire().to_string(),
        runtime: family.runtime().to_string(),
        engine: proc.name.clone(),
        load_ms: proc.load_ms,
        infer_ms: resp.ms,
        rtf: resp.ms as f64 / audio_ms.max(1.0),
        wer: wer.wer(),
        transcript: text,
    })
}

/// Transcribes an uploaded clip (already a 16 kHz mono WAV) with the warm
/// process of `model` (D167 "test by dropping an audio").
pub async fn test_clip(
    ctx: &AppContext,
    model: &ml_models::Model,
    wav: &[u8],
    language: &str,
    truncated: bool,
) -> Result<pnex_core::media_ingest::AsrTestResult, AsrError> {
    let settings = AsrSettings::from_env();
    let local = local_model(ctx, &settings, model.asset_id, model.asset_version).await?;
    let family = Family::from_wire(&model.family).unwrap_or(local.inspection.family);
    let launch = launch(&settings, &local, family, language)?;
    let resp = process::transcribe(&launch, &settings.sandbox, wav).await?;
    Ok(pnex_core::media_ingest::AsrTestResult {
        text: resp.text.unwrap_or_default(),
        words: resp
            .words
            .into_iter()
            .map(|w| pnex_core::media_ingest::AsrTestWord {
                word: w.w,
                start_ms: w.s,
                end_ms: w.e,
            })
            .collect(),
        audio_ms: (wav.len().saturating_sub(44) / 32) as u64,
        infer_ms: resp.ms,
        truncated,
    })
}
