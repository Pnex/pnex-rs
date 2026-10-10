//! `pnex-asr serve` processes (media-ingest.md D166): one per loaded model,
//! kept warm between segments, confined like the decoder (no network,
//! read-only filesystem), restarted on demand after a crash.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use pnex_asr::protocol::{Fatal, Ready, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use crate::services::media_ingest::capture::sandbox::{self, SandboxMode};

/// Model load bound (large Whisper on a Pi).
const LOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// One segment bound.
const INFER_TIMEOUT: Duration = Duration::from_secs(300);
/// A process unused this long is stopped.
const IDLE: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AsrProcessError {
    #[error("pnex-asr not found")]
    Missing,
    #[error("model load failed: {0}")]
    Load(String),
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("pnex-asr died")]
    Died,
    #[error("pnex-asr timed out")]
    Timeout,
}

/// How to start one model.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Launch {
    pub bin: PathBuf,
    pub dir: PathBuf,
    pub family: String,
    pub language: String,
    pub threads: u32,
    pub provider: String,
    /// `RLIMIT_AS` of the process, bytes (0 = none).
    pub max_memory: u64,
    /// VAD run before the transcription (D166).
    pub vad_dir: Option<PathBuf>,
    /// Diarization: segmentation model; needs `emb_dir`.
    pub seg_dir: Option<PathBuf>,
    /// Speaker embedding model (diarization, or a segmentation check).
    pub emb_dir: Option<PathBuf>,
}

pub struct AsrProcess {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    pub name: String,
    pub load_ms: u64,
    last_used: Instant,
}

impl AsrProcess {
    pub async fn start(launch: &Launch, mode: &SandboxMode) -> Result<Self, AsrProcessError> {
        let max_memory = launch.max_memory;
        let mut argv = vec![
            launch.bin.display().to_string(),
            "serve".into(),
            "--dir".into(),
            launch.dir.display().to_string(),
            "--family".into(),
            launch.family.clone(),
            "--language".into(),
            launch.language.clone(),
            "--threads".into(),
            launch.threads.to_string(),
            "--provider".into(),
            launch.provider.clone(),
        ];
        if launch.provider != "cpu" {
            argv.push("--gpu".into());
        }
        for (flag, dir) in [
            ("--vad-dir", &launch.vad_dir),
            ("--seg-dir", &launch.seg_dir),
            ("--emb-dir", &launch.emb_dir),
        ] {
            if let Some(d) = dir {
                argv.push(flag.into());
                argv.push(d.display().to_string());
            }
        }
        let argv = sandbox::wrap_argv(mode, argv);
        let mut cmd = tokio::process::Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        sandbox::confine(&mut cmd, mode, max_memory);
        let mut child = cmd.spawn().map_err(|_| AsrProcessError::Missing)?;
        let stdin = child.stdin.take().ok_or(AsrProcessError::Died)?;
        let mut stdout = BufReader::new(child.stdout.take().ok_or(AsrProcessError::Died)?);
        let mut line = String::new();
        match tokio::time::timeout(LOAD_TIMEOUT, stdout.read_line(&mut line)).await {
            Err(_) => return Err(AsrProcessError::Timeout),
            Ok(Err(_)) | Ok(Ok(0)) => return Err(AsrProcessError::Died),
            Ok(Ok(_)) => {}
        }
        if let Ok(ready) = serde_json::from_str::<Ready>(&line) {
            return Ok(Self {
                _child: child,
                stdin,
                stdout,
                next_id: 1,
                name: ready.ready,
                load_ms: ready.load_ms,
                last_used: Instant::now(),
            });
        }
        let reason = serde_json::from_str::<Fatal>(&line)
            .map(|f| f.fatal)
            .unwrap_or_else(|_| "unexpected output".into());
        Err(AsrProcessError::Load(reason))
    }

    pub async fn transcribe(&mut self, wav: &[u8]) -> Result<Response, AsrProcessError> {
        self.last_used = Instant::now();
        let id = self.next_id;
        self.next_id += 1;
        let req = Request {
            id,
            wav_b64: base64::engine::general_purpose::STANDARD.encode(wav),
        };
        let mut line = serde_json::to_vec(&req).map_err(|_| AsrProcessError::Died)?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .map_err(|_| AsrProcessError::Died)?;
        let mut out = String::new();
        match tokio::time::timeout(INFER_TIMEOUT, self.stdout.read_line(&mut out)).await {
            Err(_) => return Err(AsrProcessError::Timeout),
            Ok(Err(_)) | Ok(Ok(0)) => return Err(AsrProcessError::Died),
            Ok(Ok(_)) => {}
        }
        let resp: Response = serde_json::from_str(&out).map_err(|_| AsrProcessError::Died)?;
        if resp.id != id {
            return Err(AsrProcessError::Died);
        }
        match &resp.error {
            Some(e) => Err(AsrProcessError::Inference(e.clone())),
            None => Ok(resp),
        }
    }
}

type Slot = Arc<Mutex<Option<AsrProcess>>>;

fn pool() -> &'static std::sync::Mutex<HashMap<Launch, Slot>> {
    static POOL: OnceLock<std::sync::Mutex<HashMap<Launch, Slot>>> = OnceLock::new();
    POOL.get_or_init(Default::default)
}

/// Transcribes with the warm process of `launch` (started on first use,
/// restarted after a crash). One segment at a time per process.
pub async fn transcribe(
    launch: &Launch,
    mode: &SandboxMode,
    wav: &[u8],
) -> Result<Response, AsrProcessError> {
    let slot = {
        let mut map = pool().lock().unwrap_or_else(|e| e.into_inner());
        evict_idle(&mut map);
        map.entry(launch.clone()).or_default().clone()
    };
    let mut guard = slot.lock().await;
    if guard.is_none() {
        *guard = Some(AsrProcess::start(launch, mode).await?);
    }
    let result = match guard.as_mut() {
        Some(p) => p.transcribe(wav).await,
        None => Err(AsrProcessError::Died),
    };
    if matches!(
        result,
        Err(AsrProcessError::Died | AsrProcessError::Timeout)
    ) {
        // Killed on drop; the next segment starts a fresh process.
        *guard = None;
    }
    result
}

/// Drops processes idle for [`IDLE`] (killed on drop).
fn evict_idle(map: &mut HashMap<Launch, Slot>) {
    map.retain(|_, slot| match slot.try_lock() {
        Ok(guard) => guard.as_ref().is_some_and(|p| p.last_used.elapsed() < IDLE),
        // In use right now.
        Err(_) => true,
    });
}

/// Stops the processes of a model directory (model deleted or replaced).
pub fn forget(dir: &Path) {
    let mut map = pool().lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|k, _| k.dir != dir);
}
