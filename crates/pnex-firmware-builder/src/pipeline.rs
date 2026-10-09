//! Pipeline de build : extraction de la source embarquée → `pio run` →
//! merge-bin → ArtifactStore.
//!
//! La source du firmware est **l'arborescence embarquée dans le binaire**
//! ([`crate::embedded`] — convergence monorepo) : une version du serveur
//! compile exactement la version du firmware qui l'accompagne, sans
//! sélecteur, clone git ni chemin local.
//!
//! 1. workspace tmp par job ([`tempfile::TempDir`]) — le drop efface tout,
//!    même en erreur (les secrets sont compilés dans les artefacts
//!    intermédiaires) ;
//! 2. extraction de l'arborescence embarquée (layout complet du workspace
//!    PlatformIO — `lib_extra_dirs = ../common_libs` impose le frère
//!    `common_libs/`) ;
//! 3. `pio run` dans `{workspace}/{project}` — projet = nom du predefined
//!    device, qui doit contenir `platformio.ini` ;
//! 4. découverte `.pio/build/{env}/*.bin` puis `esptool merge-bin`
//!    (esp8266 : image unique @0x0, pas de merge) ;
//! 5. dépôt sous la clé D6 `org_{id}/firmware/{device_id}-firmware.bin`.
//!
//! Timeout dur : une **deadline globale** couvre tous les sous-process ; à
//! l'expiration les enfants sont tués (`kill_on_drop`) et le build échoue.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;

use sha2::Digest;

use crate::custom::{BuildOptions, Sandbox};
use crate::{
    artifact_key, ota_artifact_key, sanitize_segment, ArtifactStore, BuildError, BuildSecrets,
    BuildStep,
};

/// Réglages d'exécution d'un build.
#[derive(Clone)]
pub struct BuildConfig {
    /// Commande PlatformIO (`pio` ou `uv run pio` — split sur les espaces,
    /// pas de guillemets : passer par un script wrapper sinon).
    pub pio_cmd: String,
    /// Commande esptool (`esptool` ou `python -m esptool`).
    pub esptool_cmd: String,
    /// Budget global du build en secondes (défaut conseillé : 900).
    pub timeout_secs: u64,
    pub store: Arc<dyn ArtifactStore>,
}

/// Device pour lequel on compile.
#[derive(Debug, Clone)]
pub struct DeviceSpec {
    pub org_id: i64,
    pub device_id: String,
    /// Sous-répertoire du workspace firmware (= predefined_device_name).
    pub project: String,
    /// SoC du board (`mcu_boards.soc`) — pilote les offsets merge-bin.
    pub soc: String,
    /// PlatformIO board id de la variante (board figée du device) — injecté
    /// via `PNEX_PIO_BOARD` (les ini génériques font
    /// `board = ${sysenv.PNEX_PIO_BOARD}`). `None` = l'ini garde sa valeur
    /// hardcodée (soil_sensor, custom).
    pub pio_board: Option<String>,
    /// Id fil de la board (`PNEX_BOARD_NAME`, annoncé par the firmware).
    pub board_name: Option<String>,
    /// Screen compiled into the firmware (device choice, resolved server
    /// side) — gate `PNEX_SCREEN_{KIND}=1` + role pins `PNEX_SCREEN_{ROLE}`.
    /// `None` = no screen (all vars 0 / -1).
    pub screen: Option<ScreenSpec>,
    /// Firmware version stamped into the binary (`-D PNEX_FW_VERSION`,
    /// reported back in `Announce.fw`) and used as the versioned OTA
    /// artifact key — the build record id (monotonic per device).
    pub fw_version: String,
}

/// Screen to compile — kind gate + role wiring (board profile v2, resolved
/// by the backend through `BoardDetails::resolved_screen`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScreenSpec {
    /// Driver gate — « ssd1306 », « st7735 » (define `PNEX_SCREEN_{KIND}`).
    pub kind: String,
    /// Role→gpio wiring (i2c `sda`/`scl`, spi `sck`/`mosi`/`cs`/`dc`/`rst`).
    #[serde(default)]
    pub pins: Vec<(String, u16)>,
}

// ─────────────────── Sous-process avec deadline ───────────────────

/// The 9 screen env vars, ALWAYS set: kind gates 0/1 + role pins as decimal
/// gpio or `-1` when the role is not declared (PIO `${sysenv.*}` fails on a
/// missing var, so the inis can reference them unconditionally). The
/// firmware compiles a driver in only when its kind gate is `1`.
pub fn screen_env(screen: Option<&ScreenSpec>) -> Vec<(String, String)> {
    const ROLES: [&str; 7] = ["sda", "scl", "sck", "mosi", "cs", "dc", "rst"];
    let mut vars: Vec<(String, String)> = ["ssd1306", "st7735"]
        .iter()
        .map(|k| {
            (
                format!("PNEX_SCREEN_{}", k.to_ascii_uppercase()),
                u8::from(screen.is_some_and(|s| s.kind == *k)).to_string(),
            )
        })
        .collect();
    for role in ROLES {
        let gpio = screen
            .and_then(|s| s.pins.iter().find(|(r, _)| r == role))
            .map(|(_, g)| g.to_string())
            .unwrap_or_else(|| "-1".to_string());
        vars.push((format!("PNEX_SCREEN_{}", role.to_ascii_uppercase()), gpio));
    }
    vars
}

/// Env minimale des sous-process : on n'hérite JAMAIS de tout l'env du
/// serveur (fuite de secrets process vers les builds) — `PATH`/`HOME`
/// suffisent à pio/git, plus le cache PlatformIO s'il est redirigé.
fn base_env(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut vars = Vec::new();
    for name in ["PATH", "HOME", "PLATFORMIO_CORE_DIR"] {
        if let Ok(v) = std::env::var(name) {
            vars.push((name.to_string(), v));
        }
    }
    vars.extend(
        extra
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string())),
    );
    vars
}

fn apply_env(cmd: &mut Command, vars: &[(String, String)]) {
    cmd.env_clear();
    for (k, v) in vars {
        cmd.env(k, v);
    }
}

/// Lance un sous-process, le tue à la deadline (kill_on_drop : le `select!`
/// abandonne le child → kill), et refuse les sorties non nulles avec la
/// queue des dernières lignes en message.
async fn run_step(
    deadline: tokio::time::Instant,
    mut cmd: Command,
    step: BuildStep,
    label: &str,
) -> Result<Output, BuildError> {
    cmd.kill_on_drop(true);
    // Capture nécessaire pour les logs (wait_with_output ne lit que les
    // canaux piped — défaut : hérités).
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| BuildError::Tool(format!("{label} : lancement impossible : {e}")))?;
    tokio::select! {
        out = child.wait_with_output() => {
            let out = out.map_err(|e| BuildError::Tool(format!("{label} : {e}")))?;
            if !out.status.success() {
                return Err(BuildError::Tool(format!(
                    "{label} : sortie {}\n{}",
                    out.status,
                    tail(&out, 10)
                )));
            }
            tracing::info!(?step, "étape ok");
            Ok(out)
        }
        _ = tokio::time::sleep_until(deadline) => {
            tracing::warn!(?step, "deadline atteinte, sous-process tué");
            Err(BuildError::Timeout)
        }
    }
}

/// Queue des `n` dernières lignes (stdout ‖ stderr) d'une sortie enfant.
fn tail(out: &Output, n: usize) -> String {
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

// ─────────────────── Source ───────────────────

fn stage_source(workspace: &Path) -> Result<(), BuildError> {
    // Extraction de l'arborescence embarquée (bloquante, ~430 Ko) : le
    // layout complet — projet ET frères (`common_libs/`) — préserve le
    // `lib_extra_dirs` relatif de chaque platformio.ini.
    crate::embedded::extract(workspace)
}

// ─────────────────── Artefacts pio ───────────────────

/// Looks for `.pio/build/{env}/{name}` in the project (first match —
/// parity with the legacy script, which globs `**/firmware.bin`).
fn find_artifact(project: &Path, name: &str) -> Result<PathBuf, BuildError> {
    let build = project.join(".pio").join("build");
    let entries = std::fs::read_dir(&build)
        .map_err(|_| BuildError::NotFound(format!(".pio/build dans {}", project.display())))?;
    for entry in entries.flatten() {
        let candidate = entry.path().join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(BuildError::NotFound(format!(
        "{name} dans .pio/build ({})",
        project.display()
    )))
}

/// Ancre secondaire de résolution : la racine du monorepo, figée à la
/// compilation (`CARGO_MANIFEST_DIR` du builder = `crates/pnex-firmware-builder`).
/// Le worker tourne avec cwd = `crates/pnex-backend` : un chemin d'outil
/// relatif configuré depuis la racine (Taskfile) doit rester résolvable —
/// sinon tous les builds firmware échouent au spawn (leçon 2026-09-02).
const REPO_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// Un token-programme qui contient un `/` est un chemin (pas une recherche
/// PATH) : résolu en absolu contre le cwd du process, puis — sinon — contre
/// la racine du monorepo [`REPO_ROOT`]. Le sous-process tourne ensuite dans
/// le workspace tmp, où un chemin relatif ne pointerait plus rien.
fn resolve_program(token: &str) -> String {
    if !token.contains('/') {
        return token.to_string();
    }
    let try_anchor = |anchor: &Path| -> Option<String> {
        anchor
            .join(token)
            .canonicalize()
            .ok()
            .map(|p| p.display().to_string())
    };
    // 1) cwd du process (résolution historique — fixtures des tests, wrappers).
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(abs) = try_anchor(&cwd) {
            return abs;
        }
    }
    // 2) racine du monorepo — les chemins relatifs du Taskfile restent
    //    résolvables quel que soit le cwd du worker.
    if let Some(abs) = try_anchor(Path::new(REPO_ROOT)) {
        return abs;
    }
    // Introuvable des deux côtés : on le passe tel quel, l'erreur de
    // lancement du sous-process sera explicite.
    token.to_string()
}

// ─────────────────── Pipeline ───────────────────

/// Exécute un build complet. Retourne la clé + la taille de l'artefact
/// déposé dans le magasin.
pub async fn run_build(
    config: &BuildConfig,
    secrets: &BuildSecrets,
    device: &DeviceSpec,
) -> Result<crate::BuildArtifact, BuildError> {
    run_build_with(config, secrets, device, &BuildOptions::default()).await
}

/// Stages the source (embedded tree, then the custom sketch when present)
/// into a fresh workspace and returns the project dir.
fn stage_project(
    ws: &Path,
    device: &DeviceSpec,
    opts: &BuildOptions,
) -> Result<PathBuf, BuildError> {
    stage_source(ws)?;
    let project = ws.join(&device.project);
    if !project.join("platformio.ini").is_file() {
        return Err(BuildError::Source(format!(
            "projet {} introuvable (platformio.ini absent)",
            device.project
        )));
    }
    if let Some(custom) = &opts.custom {
        crate::custom::apply_custom_source(&project, custom)?;
    }
    seed_libdeps(&project_core_dir(&project), &project, &device.project)?;
    Ok(project)
}

/// Env var naming the PlatformIO core dir of the projects on the pioarduino
/// platform (ESP32 Arduino core 3.x). It is kept apart from the official
/// espressif32 core: both install `framework-arduinoespressif32` and
/// `tool-esptoolpy` under the same names and overwrite each other, so every
/// switch between the two families downloaded them again.
pub const PIOARDUINO_CORE_ENV: &str = "PNEX_PIO_CORE_DIR_PIOARDUINO";

/// Whether the project's `platformio.ini` builds on the pioarduino platform.
fn uses_pioarduino(project: &Path) -> bool {
    std::fs::read_to_string(project.join("platformio.ini")).is_ok_and(|ini| {
        ini.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with("platform") && l.contains("pioarduino/platform-espressif32")
        })
    })
}

/// PlatformIO core dir for this project: the pioarduino one when the image
/// provides it and the project uses that platform, else the worker's own.
fn project_core_dir(project: &Path) -> PathBuf {
    match std::env::var(PIOARDUINO_CORE_ENV) {
        Ok(dir) if !dir.is_empty() && uses_pioarduino(project) => PathBuf::from(dir),
        _ => pio_core_dir(),
    }
}

/// Copies the libraries baked into the image for this project
/// (`<core>/pnex-libdeps/<project>/<env>`) into the project's
/// `.pio/libdeps`. With the platforms and toolchains also baked in, a build
/// of a shipped project needs no network (air-gapped sites). A copy, not a
/// shared dir: a build never writes where another org's build reads. No
/// seed (dev machine, older image) = PlatformIO downloads as before.
fn seed_libdeps(core: &Path, project: &Path, name: &str) -> Result<(), BuildError> {
    let seed = core.join("pnex-libdeps").join(name);
    if !seed.is_dir() {
        return Ok(());
    }
    copy_tree(&seed, &project.join(".pio").join("libdeps"))
        .map_err(|e| BuildError::Source(format!("libdeps seed {}: {e}", seed.display())))
}

/// Recursive copy of `from` into `to` (dirs, files, symlinks kept as links).
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&src, &dst)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Builds the `pio run` command: secrets in the child env only (never
/// argv), board/screen/version vars, optional sandbox wrapper.
fn pio_command(
    pio_cmd: &str,
    project: &Path,
    secrets: &BuildSecrets,
    device: &DeviceSpec,
    sandbox: Option<&Sandbox>,
) -> Result<Command, BuildError> {
    // Ligne complète : `uv run pio` + "run" → `uv run pio run`.
    let mut pio_argv: Vec<String> = pio_cmd.split_whitespace().map(String::from).collect();
    if pio_argv.is_empty() {
        return Err(BuildError::Tool("pio_cmd vide".into()));
    }
    pio_argv.push("run".into());
    pio_argv[0] = resolve_program(&pio_argv[0]);
    let mut vars = crate::child_env(secrets);
    vars.extend(base_env(&[]));
    // Board variant + debug screen: interpolated via `${sysenv.*}` in the
    // platformio.ini. Screen vars are ALWAYS set (PIO `${sysenv.*}` fails
    // on a missing var): kind gates 0/1, role pins gpio or -1 when undeclared.
    if let Some(pb) = &device.pio_board {
        vars.push(("PNEX_PIO_BOARD".into(), pb.clone()));
    }
    if let Some(bn) = &device.board_name {
        vars.push(("PNEX_BOARD_NAME".into(), bn.clone()));
    }
    vars.extend(screen_env(device.screen.as_ref()));
    // Firmware version stamp: ALWAYS set (same school as the screen vars —
    // the generic inis reference `${sysenv.PNEX_FW_VERSION}` unconditionally
    // and PIO fails on a missing sysenv var).
    vars.push(("PNEX_FW_VERSION".into(), device.fw_version.clone()));
    // Core dir always explicit: it depends on the project's platform.
    let core = project_core_dir(project);
    vars.retain(|(k, _)| k != "PLATFORMIO_CORE_DIR");
    vars.push(("PLATFORMIO_CORE_DIR".into(), core.display().to_string()));
    let argv = match sandbox {
        None => pio_argv,
        Some(sb) => {
            // Inside the sandbox $HOME is not mounted: HOME points to the
            // private /tmp (the core dir is already explicit).
            vars.retain(|(k, _)| k != "HOME");
            vars.push(("HOME".into(), "/tmp".into()));
            let workspace = project.parent().unwrap_or(project);
            crate::custom::sandbox_argv(
                sb,
                &pio_argv,
                workspace,
                project,
                &core,
                &tool_dirs(&pio_argv[0]),
            )
        }
    };
    let mut pio = Command::new(&argv[0]);
    pio.args(&argv[1..]);
    pio.current_dir(project);
    apply_env(&mut pio, &vars);
    Ok(pio)
}

/// PlatformIO core dir of the worker (`PLATFORMIO_CORE_DIR`, else
/// `~/.platformio`).
fn pio_core_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PLATFORMIO_CORE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/root".into())).join(".platformio")
}

/// Read-only locations a sandboxed `pio` needs: its own dir, the venv it
/// links to (uv shim → `~/.local/share/uv/tools/platformio`) and the
/// interpreter named by the venv script shebang.
fn tool_dirs(program: &str) -> Vec<PathBuf> {
    let path = if program.contains('/') {
        Some(PathBuf::from(program))
    } else {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(program))
                .find(|p| p.is_file())
        })
    };
    let Some(path) = path else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = path.parent() {
        dirs.push(d.to_path_buf());
    }
    if let Ok(canon) = path.canonicalize() {
        // bin/pio → the venv root.
        if let Some(root) = canon.parent().and_then(Path::parent) {
            dirs.push(root.to_path_buf());
        }
        if let Ok(head) = std::fs::read_to_string(&canon) {
            if let Some(interp) = head.lines().next().and_then(|l| l.strip_prefix("#!")) {
                let interp = interp.split_whitespace().next().unwrap_or("");
                if let Ok(real) = Path::new(interp).canonicalize() {
                    if let Some(root) = real.parent().and_then(Path::parent) {
                        dirs.push(root.to_path_buf());
                    }
                }
            }
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

/// Outcome of a compile-only check (IDE « Verify », D90).
#[derive(Debug, Clone)]
pub struct CompileOutcome {
    pub ok: bool,
    pub diagnostics: Vec<pnex_core::firmware::CompileDiagnostic>,
    /// Last lines of the compiler output (verbatim runtime diagnostic).
    pub log_tail: String,
}

/// Compiles the project without merging nor storing anything (IDE check).
/// `secrets` should be placeholders: a check never needs real credentials.
/// Tool/infra failures (spawn, timeout) are errors; a compile failure is an
/// `Ok` outcome carrying the diagnostics.
pub async fn compile_check(
    pio_cmd: &str,
    timeout_secs: u64,
    secrets: &BuildSecrets,
    device: &DeviceSpec,
    opts: &BuildOptions,
) -> Result<CompileOutcome, BuildError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs.max(1));
    let workspace = tempfile::tempdir().map_err(|e| BuildError::Source(format!("tmp : {e}")))?;
    let project = stage_project(workspace.path(), device, opts)?;
    let mut pio = pio_command(pio_cmd, &project, secrets, device, opts.sandbox.as_ref())?;
    pio.kill_on_drop(true);
    pio.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = pio
        .spawn()
        .map_err(|e| BuildError::Tool(format!("pio run : lancement impossible : {e}")))?;
    let out = tokio::select! {
        out = child.wait_with_output() => out.map_err(|e| BuildError::Tool(format!("pio run : {e}")))?,
        _ = tokio::time::sleep_until(deadline) => return Err(BuildError::Timeout),
    };
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    Ok(CompileOutcome {
        ok: out.status.success(),
        diagnostics: pnex_core::firmware::parse_gcc_diagnostics(&text),
        log_tail: tail(&out, 40),
    })
}

/// Full build with options (custom sketch, sandbox).
pub async fn run_build_with(
    config: &BuildConfig,
    secrets: &BuildSecrets,
    device: &DeviceSpec,
    opts: &BuildOptions,
) -> Result<crate::BuildArtifact, BuildError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(config.timeout_secs.max(1));
    // Le drop du TempDir efface le workspace (secrets) dans tous les chemins.
    let workspace = tempfile::tempdir().map_err(|e| BuildError::Source(format!("tmp : {e}")))?;
    let ws = workspace.path();
    let project = stage_project(ws, device, opts)?;
    let pio = pio_command(
        &config.pio_cmd,
        &project,
        secrets,
        device,
        opts.sandbox.as_ref(),
    )?;
    run_step(deadline, pio, BuildStep::Compile, "pio run").await?;

    // Fusion (ou copie directe pour un SoC à image unique).
    let firmware = find_artifact(&project, "firmware.bin")?;
    let final_bin = ws.join(format!(
        "{}-firmware.bin",
        sanitize_segment(&device.device_id)
    ));
    match crate::merge_offsets(&device.soc) {
        None => {
            tokio::fs::copy(&firmware, &final_bin)
                .await
                .map_err(|e| BuildError::Tool(format!("copie firmware : {e}")))?;
        }
        Some(offsets) => {
            let inputs: Vec<(String, PathBuf)> = offsets
                .iter()
                .map(|(off, file)| Ok(((*off).to_string(), find_artifact(&project, file)?)))
                .collect::<Result<_, BuildError>>()?;
            let mut esptool_argv =
                crate::merge_args(&config.esptool_cmd, &device.soc, &final_bin, &inputs);
            esptool_argv[0] = resolve_program(&esptool_argv[0]);
            let mut esptool = Command::new(&esptool_argv[0]);
            esptool.args(&esptool_argv[1..]);
            esptool.current_dir(ws);
            apply_env(&mut esptool, &base_env(&[]));
            run_step(deadline, esptool, BuildStep::MergeBin, "esptool merge-bin").await?;
        }
    }

    // Store the merged/serial image under its D6 key (unchanged).
    let bytes = tokio::fs::read(&final_bin)
        .await
        .map_err(|e| BuildError::Tool(format!("lecture artefact : {e}")))?;
    let key = artifact_key(device.org_id, &device.device_id);
    config.store.put(&key, &bytes).await?;
    tracing::info!(step = ?BuildStep::Upload, key = %key, size = bytes.len(), "artefact déposé");

    // OTA payload = the RAW app image (pre-merge — the merged image only
    // serves serial flashing at 0x0; Update.h writes the inactive slot /
    // the eboot staging area). The sha256 digest is the device-side
    // integrity gate before `Update.end(true)` flips the boot slot.
    let ota_bytes = tokio::fs::read(&firmware)
        .await
        .map_err(|e| BuildError::Tool(format!("lecture image app : {e}")))?;
    let ota_sha256 = sha2::Sha256::digest(&ota_bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let ota_key = ota_artifact_key(device.org_id, &device.device_id, &device.fw_version);
    config.store.put(&ota_key, &ota_bytes).await?;
    tracing::info!(
        step = ?BuildStep::Upload, key = %ota_key, size = ota_bytes.len(), sha = %ota_sha256,
        "OTA artifact (raw app image) stored"
    );
    Ok(crate::BuildArtifact {
        key,
        size_bytes: bytes.len() as u64,
        ota_key,
        ota_sha256,
        ota_size_bytes: ota_bytes.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryStore;

    /// Fabrique une fausse toolchain : `pio` écrit les artefacts (avec les
    /// env reçues), `esptool` concatène les fichiers passés en offset.
    fn fake_toolchain(dir: &Path) -> (String, String) {
        let pio = dir.join("fake_pio.sh");
        std::fs::write(
            &pio,
            "#!/bin/sh\nmkdir -p .pio/build/stub\necho \"pio ssid=$WIFI_SSID host=$HOST ssl=$WS_SSL\" > .pio/build/stub/firmware.bin\necho boot > .pio/build/stub/bootloader.bin\necho part > .pio/build/stub/partitions.bin\n",
        )
        .expect("pio");
        let esptool = dir.join("fake_esptool.sh");
        std::fs::write(
            &esptool,
            "#!/bin/sh\nout=\"\"; prev=\"\"\nfor a in \"$@\"; do\n  [ \"$prev\" = \"-o\" ] && out=\"$a\"\n  prev=\"$a\"\ndone\n: > \"$out\"\nfor a in \"$@\"; do\n  if [ -f \"$a\" ] && [ \"$a\" != \"$out\" ]; then cat \"$a\" >> \"$out\"; fi\ndone\n",
        )
        .expect("esptool");
        for f in [&pio, &esptool] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        (pio.display().to_string(), esptool.display().to_string())
    }

    fn secrets() -> BuildSecrets {
        BuildSecrets {
            wifi_ssid: "coloc".into(),
            wifi_password: "w0rd".into(),
            host: "dev1.pnex.io".into(),
            ws_ssl: true,
            token: "tok".into(),
            device_id: "capteur-jardin".into(),
            encryption_key: None,
            ca_cert_pem: None,
            ota_pubkey: None,
            client_cert: None,
        }
    }

    fn device(soc: &str) -> DeviceSpec {
        DeviceSpec {
            org_id: 7,
            device_id: "capteur-jardin".into(),
            project: "soil_sensor".into(),
            soc: soc.into(),
            pio_board: None,
            board_name: None,
            screen: None,
            fw_version: "42".into(),
        }
    }

    #[test]
    fn pioarduino_projects_are_detected_from_their_ini() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ini = tmp.path().join("platformio.ini");
        std::fs::write(&ini, "[env:x]\nplatform = https://github.com/pioarduino/platform-espressif32/releases/download/55.03.312-1/platform-espressif32.zip\n").unwrap();
        assert!(uses_pioarduino(tmp.path()));
        std::fs::write(
            &ini,
            "[env:x]\nplatform = espressif32\n; pioarduino/platform-espressif32 in a comment\n",
        )
        .unwrap();
        assert!(!uses_pioarduino(tmp.path()));
        assert!(!uses_pioarduino(&tmp.path().join("missing")));
    }

    /// The baked libraries land in `.pio/libdeps`, links kept; no seed for
    /// the project is not an error.
    #[test]
    fn libdeps_seed_is_copied_into_the_project() {
        let core = tempfile::tempdir().expect("core");
        let project = tempfile::tempdir().expect("project");
        let lib = core
            .path()
            .join("pnex-libdeps/generic_esp32/esp32/ArduinoJson");
        std::fs::create_dir_all(lib.join("src")).unwrap();
        std::fs::write(lib.join("src/ArduinoJson.h"), "// lib").unwrap();
        std::fs::write(
            core.path()
                .join("pnex-libdeps/generic_esp32/esp32/integrity.dat"),
            "x",
        )
        .unwrap();
        std::os::unix::fs::symlink("src/ArduinoJson.h", lib.join("link.h")).unwrap();

        seed_libdeps(core.path(), project.path(), "generic_esp32").expect("seed");
        let dst = project.path().join(".pio/libdeps/esp32");
        assert!(dst.join("ArduinoJson/src/ArduinoJson.h").is_file());
        assert!(dst.join("integrity.dat").is_file());
        assert!(dst.join("ArduinoJson/link.h").is_symlink());

        seed_libdeps(core.path(), project.path(), "soil_sensor").expect("no seed is fine");
    }

    /// The 9 screen vars are ALWAYS set: 0/-1 without screen, gate=1 and
    /// role pins from the spec with a screen.
    #[test]
    fn screen_env_toujours_posees() {
        // no screen → all gates 0, roles -1
        let vars = super::screen_env(None);
        assert_eq!(vars.len(), 9);
        for (k, v) in &vars {
            let expected = match k.as_str() {
                "PNEX_SCREEN_SSD1306" | "PNEX_SCREEN_ST7735" => "0",
                _ => "-1",
            };
            assert_eq!(v, expected, "{k}");
        }
        // st7735 picked → gate 1 + role pins wired
        let vars = super::screen_env(Some(&ScreenSpec {
            kind: "st7735".into(),
            pins: vec![
                ("sck".into(), 14),
                ("mosi".into(), 13),
                ("cs".into(), 15),
                ("dc".into(), 4),
                ("rst".into(), 5),
            ],
        }));
        let get = |name: &str| {
            vars.iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(get("PNEX_SCREEN_SSD1306"), "0");
        assert_eq!(get("PNEX_SCREEN_ST7735"), "1");
        assert_eq!(get("PNEX_SCREEN_SCK"), "14");
        assert_eq!(get("PNEX_SCREEN_MOSI"), "13");
        assert_eq!(get("PNEX_SCREEN_CS"), "15");
        assert_eq!(get("PNEX_SCREEN_DC"), "4");
        assert_eq!(get("PNEX_SCREEN_RST"), "5");
        assert_eq!(get("PNEX_SCREEN_SDA"), "-1");
        assert_eq!(get("PNEX_SCREEN_SCL"), "-1");
    }

    /// Pipeline complet contre la fausse toolchain depuis l'arborescence
    /// embarquée, SoC esp32 : merge concatène les trois images, artefact
    /// déposé sous la clé D6, secrets propagés (WiFi clair, host base64).
    #[tokio::test]
    async fn pipeline_complet_esp32() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (pio, esptool) = fake_toolchain(tmp.path());
        let store = Arc::new(InMemoryStore::default());
        let config = BuildConfig {
            pio_cmd: pio,
            esptool_cmd: esptool,
            timeout_secs: 30,
            store: store.clone(),
        };
        let artifact = run_build(&config, &secrets(), &device("esp32"))
            .await
            .expect("build");
        assert_eq!(artifact.key, "org_7/firmware/capteur-jardin-firmware.bin");
        assert!(artifact.size_bytes > 0);
        // Versioned OTA artifact: raw app image (the fake esptool
        // concatenates bootloader+partitions+firmware — the OTA content
        // must NOT contain those contributions, proving it is pre-merge).
        assert_eq!(
            artifact.ota_key, "org_7/ota/capteur-jardin/42.bin",
            "versioned OTA key"
        );
        assert!(artifact.ota_size_bytes > 0);
        assert_eq!(artifact.ota_sha256.len(), 64);
        let ota = store.get(&artifact.ota_key).await.expect("ota get");
        assert_eq!(ota.len() as u64, artifact.ota_size_bytes);
        let ota_text = String::from_utf8_lossy(&ota);
        assert!(
            ota_text.contains("pio ssid="),
            "OTA image carries the config: {ota_text}"
        );
        assert!(
            !ota_text.contains("boot") && !ota_text.contains("part"),
            "OTA image is pre-merge (no bootloader/partitions)"
        );
        let expected_sha = sha2::Sha256::digest(&ota)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(artifact.ota_sha256, expected_sha);
        let bytes = store.get(&artifact.key).await.expect("get");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains(&format!("pio ssid={}", STANDARD.encode("coloc"))),
            "{text}"
        );
        // HOST arrive en base64 au firmware (vérifié par le fake pio).
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine as _;
        assert!(text.contains(&format!("host={}", STANDARD.encode("dev1.pnex.io"))));
        // Le schéma WebSocket transite aussi en env du sous-process.
        assert!(text.contains("ssl=true"), "{text}");
        // Le merge a bien concaténé bootloader + partitions + firmware.
        assert!(text.contains("boot") && text.contains("part"));
    }

    /// esp8266 : image unique — esptool n'est jamais appelé (commande
    /// volontairement cassée).
    #[tokio::test]
    async fn esp8266_sans_merge() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (pio, _) = fake_toolchain(tmp.path());
        let store = Arc::new(InMemoryStore::default());
        let config = BuildConfig {
            pio_cmd: pio,
            esptool_cmd: "false".into(),
            timeout_secs: 30,
            store: store.clone(),
        };
        let artifact = run_build(&config, &secrets(), &device("esp8266"))
            .await
            .expect("build sans merge");
        assert_eq!(artifact.key, "org_7/firmware/capteur-jardin-firmware.bin");
        // 8266: no merge — the raw OTA image IS the merged image.
        assert_eq!(artifact.ota_key, "org_7/ota/capteur-jardin/42.bin");
        assert_eq!(
            store.get(&artifact.ota_key).await.expect("ota get").len() as u64,
            artifact.ota_size_bytes
        );
    }

    /// Échec de compilation : exit non nul → Tool avec la queue de logs.
    #[tokio::test]
    async fn echec_outil_exit_non_nul() {
        let tmp = tempfile::tempdir().expect("tmp");
        let pio = tmp.path().join("fail_pio.sh");
        std::fs::write(&pio, "#!/bin/sh\necho erreur de compilation\nexit 1\n").expect("pio");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&pio, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let store = Arc::new(InMemoryStore::default());
        let config = BuildConfig {
            pio_cmd: pio.display().to_string(),
            esptool_cmd: "false".into(),
            timeout_secs: 30,
            store,
        };
        let err = run_build(&config, &secrets(), &device("esp8266"))
            .await
            .expect_err("échec attendue");
        assert!(
            matches!(err, BuildError::Tool(ref m) if m.contains("erreur de compilation")),
            "{err}"
        );
    }

    /// Timeout dur : sous-process endormi tué à la deadline (test borné).
    #[tokio::test]
    async fn timeout_tue_le_sous_process() {
        let tmp = tempfile::tempdir().expect("tmp");
        // Un script qui ignore ses arguments (pio ajoute « run » en argv).
        let sleeper = tmp.path().join("sleeper.sh");
        std::fs::write(&sleeper, "#!/bin/sh\nsleep 30\n").expect("sleeper");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sleeper, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let store = Arc::new(InMemoryStore::default());
        let config = BuildConfig {
            pio_cmd: sleeper.display().to_string(),
            esptool_cmd: "false".into(),
            timeout_secs: 1,
            store,
        };
        let started = std::time::Instant::now();
        let err = run_build(&config, &secrets(), &device("esp8266"))
            .await
            .expect_err("timeout attendu");
        assert!(matches!(err, BuildError::Timeout), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Un chemin relatif configuré depuis la racine du monorepo se résout
    /// même quand le cwd est ailleurs (worker : crates/pnex-backend).
    #[test]
    fn chemin_relatif_resolu_depuis_la_racine_monorepo() {
        // Existe depuis la racine du monorepo, pas depuis le cwd des tests
        // (crates/pnex-firmware-builder).
        let token = "crates/pnex-firmware-builder/Cargo.toml";
        let resolved = resolve_program(token);
        assert!(Path::new(&resolved).is_absolute(), "{resolved}");
        assert!(resolved.ends_with("Cargo.toml"), "{resolved}");
    }

    /// Un chemin introuvable (aucune ancre) passe tel quel — l'erreur de
    /// spawn du sous-process reste le signal.
    #[test]
    fn chemin_introuvable_passe_telquel() {
        let token = "n importe/quelle chemin";
        assert_eq!(resolve_program(token), token);
    }

    /// Projet absent de la source (predefined name ≠ sous-répertoire).
    #[tokio::test]
    async fn projet_introuvable() {
        let store = Arc::new(InMemoryStore::default());
        let config = BuildConfig {
            pio_cmd: "true".into(),
            esptool_cmd: "true".into(),
            timeout_secs: 10,
            store,
        };
        let mut dev = device("esp8266");
        dev.project = "inconnu".into();
        let err = run_build(&config, &secrets(), &dev)
            .await
            .expect_err("source attendue");
        assert!(
            matches!(err, BuildError::Source(ref m) if m.contains("inconnu")),
            "{err}"
        );
    }
}

#[cfg(test)]
mod real_pio_tests {
    use super::*;
    use crate::custom::CustomSource;
    use crate::InMemoryStore;

    /// Repro manuel (Ignored) : pipeline RÉEL avec pio installé sur la
    /// machine (`cargo test -p pnex-firmware-builder manuel_pio -- --ignored`).
    /// Reproduit exactement le chemin du worker BuildFirmwareWorker.
    /// Manual (real pio, registry reachable): custom sketch + a catalog
    /// library compiles; a broken sketch yields diagnostics on main.cpp.
    /// `cargo test -p pnex-firmware-builder -- --ignored manuel_compile_check`
    #[tokio::test]
    #[ignore = "real PlatformIO toolchain"]
    async fn manuel_compile_check_custom_sketch() {
        let secrets = BuildSecrets {
            wifi_ssid: "check".into(),
            wifi_password: "check".into(),
            host: "localhost:5150".into(),
            ws_ssl: false,
            token: "check".into(),
            device_id: "check".into(),
            encryption_key: None,
            ca_cert_pem: None,
            ota_pubkey: None,
            client_cert: None,
        };
        let device = DeviceSpec {
            org_id: 1,
            device_id: "check".into(),
            project: "generic_esp8266".into(),
            soc: "esp8266".into(),
            pio_board: Some("nodemcuv2".into()),
            board_name: Some("nodemcu".into()),
            screen: None,
            fw_version: "0".into(),
        };
        let sketch = pnex_core::firmware::STARTER_SKETCH.replace(
            "#include <Pnex.h>",
            "#include <Pnex.h>\n#include <DHT.h>\nDHT dht(4, DHT22);",
        );
        let lib = pnex_core::firmware::lib_by_id("dht")
            .unwrap()
            .pio_spec
            .to_string();
        let opts = BuildOptions {
            custom: Some(CustomSource {
                main_cpp: sketch,
                lib_specs: vec![lib.clone()],
            }),
            sandbox: None,
        };
        let ok = compile_check("pio", 900, &secrets, &device, &opts)
            .await
            .expect("check");
        assert!(ok.ok, "{}", ok.log_tail);

        let broken = BuildOptions {
            custom: Some(CustomSource {
                main_cpp: "#include <Pnex.h>\nvoid setup() { undefined_call(); }\nvoid loop() {}\n"
                    .into(),
                lib_specs: vec![],
            }),
            sandbox: None,
        };
        let ko = compile_check("pio", 900, &secrets, &device, &broken)
            .await
            .expect("check");
        assert!(!ko.ok);
        let d = ko
            .diagnostics
            .iter()
            .find(|d| d.in_sketch && d.severity == "error")
            .expect("sketch error");
        assert_eq!(d.line, 2, "{:?}", ko.diagnostics);
    }

    #[tokio::test]
    #[ignore]
    async fn manuel_pio_reel_generic_esp8266() {
        let store = Arc::new(InMemoryStore::default());
        let config = BuildConfig {
            pio_cmd: "pio".into(),
            esptool_cmd: "esptool".into(),
            timeout_secs: 900,
            store,
        };
        let secrets = BuildSecrets {
            wifi_ssid: "test-wifi".into(),
            wifi_password: "test-pass".into(),
            host: "localhost:5150".into(),
            ws_ssl: false,
            token: "fake-token".into(),
            device_id: "young-walrus".into(),
            encryption_key: None,
            ca_cert_pem: None,
            ota_pubkey: None,
            client_cert: None,
        };
        let device = DeviceSpec {
            org_id: 1,
            device_id: "young-walrus".into(),
            project: "generic_esp8266".into(),
            soc: "esp8266".into(),
            // Preuve de l'interpolation ${sysenv.PNEX_PIO_BOARD} sur la
            // ligne `board =` : sans la var, pio échoue à la config.
            pio_board: Some("nodemcuv2".into()),
            board_name: Some("nodemcu".into()),
            screen: None,
            fw_version: "42".into(),
        };
        let artifact = run_build(&config, &secrets, &device)
            .await
            .expect("build réel doit passer");
        assert!(artifact.size_bytes > 100_000, "bin trop petit ?");
    }

    /// Miroir esp32-c3 (Seeed XIAO) : platform espressif32 + toolchain riscv
    /// requis ; le merge esptool --chip esp32c3 produit l'image mergée
    /// (bootloader+partitions+app) — chemin exact du worker.
    #[tokio::test]
    #[ignore]
    async fn manuel_pio_reel_generic_esp32c3() {
        let store = Arc::new(InMemoryStore::default());
        let config = BuildConfig {
            pio_cmd: "pio".into(),
            esptool_cmd: "esptool".into(),
            timeout_secs: 900,
            store,
        };
        let secrets = BuildSecrets {
            wifi_ssid: "test-wifi".into(),
            wifi_password: "test-pass".into(),
            host: "localhost:5150".into(),
            ws_ssl: false,
            token: "fake-token".into(),
            device_id: "young-walrus".into(),
            encryption_key: None,
            ca_cert_pem: None,
            ota_pubkey: None,
            client_cert: None,
        };
        let device = DeviceSpec {
            org_id: 1,
            device_id: "young-walrus".into(),
            project: "generic_esp32c3".into(),
            soc: "esp32-c3".into(),
            pio_board: Some("seeed_xiao_esp32c3".into()),
            board_name: Some("xiao_esp32c3".into()),
            screen: None,
            fw_version: "42".into(),
        };
        let artifact = run_build(&config, &secrets, &device)
            .await
            .expect("build réel C3 doit passer");
        // Image mergée C3 = bootloader + partitions + app.
        assert!(artifact.size_bytes > 400_000, "bin trop petit ?");
    }
}
