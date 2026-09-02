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

use crate::{artifact_key, sanitize_segment, ArtifactStore, BuildError, BuildSecrets, BuildStep};

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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(config.timeout_secs.max(1));
    // Le drop du TempDir efface le workspace (secrets) dans tous les chemins.
    let workspace = tempfile::tempdir().map_err(|e| BuildError::Source(format!("tmp : {e}")))?;
    let ws = workspace.path();

    stage_source(ws)?;

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
    use crate::InMemoryStore;

    /// Repro manuel (Ignored) : pipeline RÉEL avec pio installé sur la
    /// machine (`cargo test -p pnex-firmware-builder manuel_pio -- --ignored`).
    /// Reproduit exactement le chemin du worker BuildFirmwareWorker.
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
        };
        let device = DeviceSpec {
            org_id: 1,
            device_id: "young-walrus".into(),
            project: "generic_esp8266".into(),
            soc: "esp8266".into(),
        };
        let artifact = run_build(&config, &secrets, &device)
            .await
            .expect("build réel doit passer");
        assert!(artifact.size_bytes > 100_000, "bin trop petit ?");
    }
}
