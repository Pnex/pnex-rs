//! Pipeline de build : source → `pio run` → merge-bin → ArtifactStore.
//!
//! Parité du script k8s_job Django, transposé en sous-process locaux :
//!
//! 1. workspace tmp par job ([`tempfile::TempDir`]) — le drop efface tout,
//!    même en erreur (les secrets sont compilés dans les artefacts
//!    intermédiaires) ;
//! 2. mise en place de la source : copie locale (l'arborescence complète du
//!    workspace PlatformIO — `lib_extra_dirs = ../common_libs` impose le
//!    frère `common_libs/`) ou `git clone --depth 1 --branch {ref}` ;
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

use crate::{artifact_key, sanitize_segment, ArtifactStore, BuildError, BuildSecrets, BuildStep};

/// Source du firmware : arborescence PlatformIO locale (dev/edge) ou dépôt
/// git (cloud). Le clone est `--depth 1 --branch {ref}` — branche/tag
/// uniquement, pas de SHA (limite documentée).
#[derive(Debug, Clone)]
pub enum FirmwareSource {
    Local { path: PathBuf },
    Git { repo: String, git_ref: String },
}

/// Réglages d'exécution d'un build.
#[derive(Clone)]
pub struct BuildConfig {
    pub source: FirmwareSource,
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
}

// ─────────────────── Sous-process avec deadline ───────────────────

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
    vars.extend(extra.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())));
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
    let text = String::from_utf8_lossy(&out.stdout).into_owned()
        + &String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

// ─────────────────── Source ───────────────────

/// Copie récursive (std::fs, bloquant : l'arborescence firmware sans
/// `.pio`/`.git` fait quelques centaines de Ko). Préserve le layout
/// complet — le projet ET ses frères (`common_libs/`).
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), ".git" | ".pio" | "node_modules") {
            continue;
        }
        let from = entry.path();
        let to = dst.join(name.as_ref());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

async fn stage_source(
    source: &FirmwareSource,
    workspace: &Path,
    deadline: tokio::time::Instant,
) -> Result<(), BuildError> {
    match source {
        FirmwareSource::Local { path } => {
            if !path.is_dir() {
                return Err(BuildError::Source(format!(
                    "chemin source introuvable : {}",
                    path.display()
                )));
            }
            copy_dir_recursive(path, workspace).map_err(|e| {
                BuildError::Source(format!("copie de {} : {e}", path.display()))
            })?;
            Ok(())
        }
        FirmwareSource::Git { repo, git_ref } => {
            let mut cmd = Command::new("git");
            cmd.args([
                "clone",
                "--depth",
                "1",
                "--branch",
                git_ref,
                repo,
                &workspace.display().to_string(),
            ]);
            apply_env(
                &mut cmd,
                &base_env(&[("GIT_TERMINAL_PROMPT", "0")]),
            );
            run_step(deadline, cmd, BuildStep::Clone, &format!("git clone {repo}")).await?;
            Ok(())
        }
    }
}

// ─────────────────── Artefacts pio ───────────────────

/// Cherche `.pio/build/{env}/{name}` dans le projet (premier trouvé —
/// parité du script Django, qui cherche `**/firmware.bin`).
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

/// Un token-programme qui contient un `/` est un chemin (pas une recherche
/// PATH) : on le résout en absolu contre le CWD du process — le sous-process
/// tourne ensuite dans le workspace, où un chemin relatif ne pointerait
/// plus rien (fixtures des tests, wrappers locaux).
fn resolve_program(token: &str) -> String {
    if !token.contains('/') {
        return token.to_string();
    }
    let path = std::path::Path::new(token);
    match std::env::current_dir()
        .map_err(|e| e.to_string())
        .and_then(|cwd| {
            cwd.join(path).canonicalize().map_err(|e| e.to_string())
        })
    {
        Ok(abs) => abs.display().to_string(),
        // Introuvable : on le passe tel quel, l'erreur de lancement du
        // sous-process sera explicite.
        Err(_) => token.to_string(),
    }
}

// ─────────────────── Pipeline ───────────────────

/// Exécute un build complet. Retourne la clé + la taille de l'artefact
/// déposé dans le magasin.
pub async fn run_build(
    config: &BuildConfig,
    secrets: &BuildSecrets,
    device: &DeviceSpec,
) -> Result<crate::BuildArtifact, BuildError> {
    let deadline =
        tokio::time::Instant::now() + Duration::from_secs(config.timeout_secs.max(1));
    // Le drop du TempDir efface le workspace (secrets) dans tous les chemins.
    let workspace = tempfile::tempdir().map_err(|e| BuildError::Source(format!("tmp : {e}")))?;
    let ws = workspace.path();

    stage_source(&config.source, ws, deadline).await?;

    let project = ws.join(&device.project);
    if !project.join("platformio.ini").is_file() {
        return Err(BuildError::Source(format!(
            "projet {} introuvable (platformio.ini absent)",
            device.project
        )));
    }

    // pio run — secrets en env du child uniquement (jamais argv).
    // Ligne complète : `uv run pio` + "run" → `uv run pio run`.
    let mut pio_argv: Vec<String> = config
        .pio_cmd
        .split_whitespace()
        .map(String::from)
        .collect();
    if pio_argv.is_empty() {
        return Err(BuildError::Tool("pio_cmd vide".into()));
    }
    pio_argv.push("run".into());
    pio_argv[0] = resolve_program(&pio_argv[0]);
    let mut pio = Command::new(&pio_argv[0]);
    pio.args(&pio_argv[1..]);
    pio.current_dir(&project);
    let mut vars = crate::child_env(secrets);
    vars.extend(base_env(&[]));
    apply_env(&mut pio, &vars);
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
            let mut esptool_argv = crate::merge_args(
                &config.esptool_cmd,
                &device.soc,
                &final_bin,
                &inputs,
            );
            esptool_argv[0] = resolve_program(&esptool_argv[0]);
            let mut esptool = Command::new(&esptool_argv[0]);
            esptool.args(&esptool_argv[1..]);
            esptool.current_dir(ws);
            apply_env(&mut esptool, &base_env(&[]));
            run_step(deadline, esptool, BuildStep::MergeBin, "esptool merge-bin").await?;
        }
    }

    // Dépôt dans le magasin.
    let bytes = tokio::fs::read(&final_bin)
        .await
        .map_err(|e| BuildError::Tool(format!("lecture artefact : {e}")))?;
    let key = artifact_key(device.org_id, &device.device_id);
    config.store.put(&key, &bytes).await?;
    tracing::info!(step = ?BuildStep::Upload, key = %key, size = bytes.len(), "artefact déposé");
    Ok(crate::BuildArtifact {
        key,
        size_bytes: bytes.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalStore;

    /// Fabrique une fausse toolchain : `pio` écrit les artefacts (avec les
    /// env reçues), `esptool` concatène les fichiers passés en offset.
    fn fake_toolchain(dir: &Path) -> (String, String) {
        let pio = dir.join("fake_pio.sh");
        std::fs::write(
            &pio,
            "#!/bin/sh\nmkdir -p .pio/build/stub\necho \"pio ssid=$WIFI_SSID host=$HOST\" > .pio/build/stub/firmware.bin\necho boot > .pio/build/stub/bootloader.bin\necho part > .pio/build/stub/partitions.bin\n",
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
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        (pio.display().to_string(), esptool.display().to_string())
    }

    /// Source locale : projet + frère common_libs (layout imposé par
    /// `lib_extra_dirs`), plus des dossiers à ignorer.
    fn fake_source(dir: &Path) -> PathBuf {
        let src = dir.join("source");
        let project = src.join("soil_sensor");
        std::fs::create_dir_all(project.join("src")).expect("dirs");
        std::fs::create_dir_all(src.join("common_libs").join("config")).expect("dirs");
        std::fs::write(project.join("platformio.ini"), "[env:soil]").expect("ini");
        std::fs::write(project.join("src").join("main.cpp"), "void setup(){}").expect("cpp");
        std::fs::write(src.join("common_libs").join("config").join("c.h"), "#pragma").expect("h");
        std::fs::create_dir_all(src.join(".git").join("objects")).expect("git");
        std::fs::write(src.join(".git").join("objects").join("x"), "secret").expect("git");
        src
    }

    fn secrets() -> BuildSecrets {
        BuildSecrets {
            wifi_ssid: "coloc".into(),
            wifi_password: "w0rd".into(),
            host: "dev1.pnex.io".into(),
            token: "tok".into(),
            device_id: "capteur-jardin".into(),
            encryption_key: None,
        }
    }

    fn device(soc: &str) -> DeviceSpec {
        DeviceSpec {
            org_id: 7,
            device_id: "capteur-jardin".into(),
            project: "soil_sensor".into(),
            soc: soc.into(),
        }
    }

    /// Copie locale : layout préservé (projet + common_libs frère),
    /// `.git`/`.pio` ignorés.
    #[tokio::test]
    async fn copie_locale_preserve_le_layout() {
        let tmp = tempfile::tempdir().expect("tmp");
        let src = fake_source(tmp.path());
        let dst = tmp.path().join("ws");
        copy_dir_recursive(&src, &dst).expect("copie");
        assert!(dst.join("soil_sensor/platformio.ini").is_file());
        assert!(dst.join("common_libs/config/c.h").is_file());
        assert!(!dst.join(".git").exists());
    }

    /// Pipeline complet contre la fausse toolchain, SoC esp32 : merge
    /// concatène les trois images, artefact déposé sous la clé D6, secrets
    /// propagés (WiFi clair, host base64).
    #[tokio::test]
    async fn pipeline_complet_esp32() {
        let tmp = tempfile::tempdir().expect("tmp");
        let src = fake_source(tmp.path());
        let (pio, esptool) = fake_toolchain(tmp.path());
        let store = Arc::new(LocalStore::new(tmp.path().join("artifacts")).expect("store"));
        let config = BuildConfig {
            source: FirmwareSource::Local { path: src },
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
        let bytes = store.get(&artifact.key).await.expect("get");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("pio ssid=coloc"), "{text}");
        // HOST arrive en base64 au firmware (vérifié par le fake pio).
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine as _;
        assert!(text.contains(&format!("host={}", STANDARD.encode("dev1.pnex.io"))));
        // Le merge a bien concaténé bootloader + partitions + firmware.
        assert!(text.contains("boot") && text.contains("part"));
    }

    /// esp8266 : image unique — esptool n'est jamais appelé (commande
    /// volontairement cassée).
    #[tokio::test]
    async fn esp8266_sans_merge() {
        let tmp = tempfile::tempdir().expect("tmp");
        let src = fake_source(tmp.path());
        let (pio, _) = fake_toolchain(tmp.path());
        let store = Arc::new(LocalStore::new(tmp.path().join("artifacts")).expect("store"));
        let config = BuildConfig {
            source: FirmwareSource::Local { path: src },
            pio_cmd: pio,
            esptool_cmd: "false".into(),
            timeout_secs: 30,
            store,
        };
        let artifact = run_build(&config, &secrets(), &device("esp8266"))
            .await
            .expect("build sans merge");
        assert_eq!(artifact.key, "org_7/firmware/capteur-jardin-firmware.bin");
    }

    /// Échec de compilation : exit non nul → Tool avec la queue de logs.
    #[tokio::test]
    async fn echec_outil_exit_non_nul() {
        let tmp = tempfile::tempdir().expect("tmp");
        let src = fake_source(tmp.path());
        let pio = tmp.path().join("fail_pio.sh");
        std::fs::write(&pio, "#!/bin/sh\necho erreur de compilation\nexit 1\n").expect("pio");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&pio, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let store = Arc::new(LocalStore::new(tmp.path().join("artifacts")).expect("store"));
        let config = BuildConfig {
            source: FirmwareSource::Local { path: src },
            pio_cmd: pio.display().to_string(),
            esptool_cmd: "false".into(),
            timeout_secs: 30,
            store,
        };
        let err = run_build(&config, &secrets(), &device("esp8266"))
            .await
            .expect_err("échec attendue");
        assert!(matches!(err, BuildError::Tool(ref m) if m.contains("erreur de compilation")), "{err}");
    }

    /// Timeout dur : sous-process endormi tué à la deadline (test borné).
    #[tokio::test]
    async fn timeout_tue_le_sous_process() {
        let tmp = tempfile::tempdir().expect("tmp");
        let src = fake_source(tmp.path());
        // Un script qui ignore ses arguments (pio ajoute « run » en argv).
        let sleeper = tmp.path().join("sleeper.sh");
        std::fs::write(&sleeper, "#!/bin/sh\nsleep 30\n").expect("sleeper");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sleeper, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
        let store = Arc::new(LocalStore::new(tmp.path().join("artifacts")).expect("store"));
        let config = BuildConfig {
            source: FirmwareSource::Local { path: src },
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

    /// Projet absent de la source (predefined name ≠ sous-répertoire).
    #[tokio::test]
    async fn projet_introuvable() {
        let tmp = tempfile::tempdir().expect("tmp");
        let src = fake_source(tmp.path());
        let store = Arc::new(LocalStore::new(tmp.path().join("artifacts")).expect("store"));
        let config = BuildConfig {
            source: FirmwareSource::Local { path: src },
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
        assert!(matches!(err, BuildError::Source(ref m) if m.contains("inconnu")), "{err}");
    }

    /// Source git : clone --depth 1 d'un dépôt fabriqué pour le test
    /// (ignoré silencieusement si git n'est pas disponible).
    #[tokio::test]
    async fn source_git_clone() {
        if !Command::new("git")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }
        let tmp = tempfile::tempdir().expect("tmp");
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(origin.join("soil_sensor")).expect("dirs");
        std::fs::write(origin.join("soil_sensor").join("platformio.ini"), "[env:x]").expect("ini");
        let run = |args: &[&str], cwd: &Path| {
            let mut c = std::process::Command::new("git");
            c.args(args).current_dir(cwd);
            c.output().expect("git")
        };
        assert!(run(&["init", "-q", "-b", "main"], &origin).status.success());
        assert!(run(&["add", "."], &origin).status.success());
        assert!(
            run(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "x"], &origin)
                .status
                .success()
        );

        let (pio, esptool) = fake_toolchain(tmp.path());
        let store = Arc::new(LocalStore::new(tmp.path().join("artifacts")).expect("store"));
        let config = BuildConfig {
            source: FirmwareSource::Git {
                repo: origin.display().to_string(),
                git_ref: "main".into(),
            },
            pio_cmd: pio,
            esptool_cmd: esptool,
            timeout_secs: 30,
            store,
        };
        let artifact = run_build(&config, &secrets(), &device("esp8266"))
            .await
            .expect("build git");
        assert_eq!(artifact.key, "org_7/firmware/capteur-jardin-firmware.bin");
    }
}
