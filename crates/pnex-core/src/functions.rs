//! Fonctions utilisateur versionnées (registre « Fonctions », groupe
//! « Automation ») — types partagés backend ↔ frontend ↔ runtime (CLI de
//! test), parseur de directives d'interface et codegen du wrapper JS.
//!
//! Contraintes (règles du crate) : pur serde + `serde_json`, aucune
//! dépendance native — compile natif et `wasm32` (le front appelle
//! `parse_directives` en wasm pour la preview live de la signature).
//!
//! Contrat d'exécution commun JS/Starlark : le code utilisateur définit
//! `handle` — JS `function handle(inputs, msg)`, Starlark
//! `def handle(inputs, msg):` — `inputs` = objet construit depuis les
//! entrées déclarées (routage par fil : `msg.topic` nommant une entrée →
//! payload entrant ; sinon lecture implicite `payload.<name>`), `msg` =
//! message Node-RED entrant. Retour (contrat clé→valeur) :
//! - `null`/`None` → message jeté (sémantique Node-RED) ;
//! - **objet + `@output` déclarés → routage clé→port** : chaque clé nommant
//!   une sortie déclarée émet un msg sur ce port (payload = la valeur) ;
//!   clé absente ou `null` = port muet ;
//! - objet sans sorties déclarées → msg sortant = msg entrant avec
//!   `payload` remplacé (fusionné si l'objet porte `payload`), port 0 ;
//! - tableau → un élément par port (règle payload-or-msg par élément ;
//!   élément `null` = port muet) ;
//! - valeur nue → port 0.
//!
//! Décisions (2026-09-15) : JS + Starlark, pas de Python (sandbox) ; JS
//! réutilise le nœud `function` QuickJS du vendor (wrapper généré), Starlark
//! = nœud custom `pnex-starlark` ; l'isolation des fonctions « bizarres »
//! est un sujet d'infra (1 tenant = 1 process), pas in-process.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Langage d'une fonction du registre — `"js"` ou `"starlark"` (VARCHAR
/// applicatif côté DB, pas d'enum PG).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FunctionLanguage {
    #[default]
    Js,
    Starlark,
}

impl FunctionLanguage {
    /// Nombre de ports de sortie du nœud de flow correspondant : au moins 1
    /// port (Node-RED : un nœud a toujours ≥ 1 sortie), même sans `@output`.
    pub fn output_count(outputs: &[FunctionOutput]) -> usize {
        std::cmp::max(1, outputs.len())
    }
}

impl std::fmt::Display for FunctionLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FunctionLanguage::Js => write!(f, "js"),
            FunctionLanguage::Starlark => write!(f, "starlark"),
        }
    }
}

/// Type déclaré d'une entrée/sortie — pilote le formulaire du panneau de
/// test typé (number → champ numérique, bool → case…) et la documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FunctionType {
    Number,
    String,
    Bool,
    Any,
}

/// Entrée déclarée par `// @input name type[=défaut] ["description"]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionInput {
    pub name: String,
    pub ty: FunctionType,
    /// Valeur par défaut quand le chemin binding est absent du msg entrant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
}

/// Sortie déclarée par `// @output name type ["description"]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionOutput {
    pub name: String,
    pub ty: FunctionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
}

/// Interface déclarée d'une fonction — extraite des directives au save,
/// stockée structurée sur `function_versions.inputs` / `.outputs`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FunctionSignature {
    #[serde(default)]
    pub inputs: Vec<FunctionInput>,
    #[serde(default)]
    pub outputs: Vec<FunctionOutput>,
}

/// Erreur de parsing de directives (400 au save, erreur de ligne au front).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectiveError {
    /// Ligne 1-based dans le code source.
    pub line: u32,
    pub message: String,
}

impl std::fmt::Display for DirectiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ligne {} : {}", self.line, self.message)
    }
}

impl std::error::Error for DirectiveError {}

// ───────────────────────── Parseur de directives ─────────────────────────

/// Nom de variable/admettant `[a-z_][a-z0-9_]*` (identifiants valides dans
/// les deux langages cibles).
fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_lowercase() => {
            chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        }
        _ => false,
    }
}

fn parse_type(tok: &str, line: u32) -> Result<FunctionType, DirectiveError> {
    match tok {
        "number" => Ok(FunctionType::Number),
        "string" => Ok(FunctionType::String),
        "bool" => Ok(FunctionType::Bool),
        "any" => Ok(FunctionType::Any),
        other => Err(DirectiveError {
            line,
            message: format!("type inconnu « {other} » (number|string|bool|any)"),
        }),
    }
}

/// Convertit le texte de défaut selon le type déclaré (pas d'espaces dans
/// un défaut : une directive = une ligne, champs séparés par des espaces).
fn parse_default(
    ty: FunctionType,
    text: &str,
    line: u32,
) -> Result<serde_json::Value, DirectiveError> {
    let bad = |message: String| DirectiveError { line, message };
    match ty {
        FunctionType::Number => text
            .parse::<f64>()
            .map(|n| serde_json::json!(n))
            .map_err(|_| bad(format!("défaut « {text} » : nombre attendu"))),
        FunctionType::String => Ok(serde_json::Value::String(
            text.trim_matches('"').to_string(),
        )),
        FunctionType::Bool => match text {
            "true" => Ok(serde_json::Value::Bool(true)),
            "false" => Ok(serde_json::Value::Bool(false)),
            _ => Err(bad(format!(
                "défaut « {text} » : bool attendu (true|false)"
            ))),
        },
        FunctionType::Any => serde_json::from_str(text)
            .map_err(|e| bad(format!("défaut « {text} » : JSON invalide ({e})"))),
    }
}

/// Retire les guillemets enveloppants d'une description optionnelle.
fn clean_desc(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let unquoted = if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    };
    if unquoted.is_empty() {
        None
    } else {
        Some(unquoted.to_string())
    }
}

/// Extrait la signature déclarée par les directives `@input`/`@output` en
/// commentaire (`//` ou `#`, tolérant pour les deux langages). Les lignes
/// qui ne sont pas des directives sont ignorées ; erreurs typées avec la
/// ligne fautive (400 au save, aperçu au front).
pub fn parse_directives(code: &str) -> Result<FunctionSignature, DirectiveError> {
    let mut sig = FunctionSignature::default();
    let mut seen_inputs: std::collections::BTreeSet<String> = Default::default();
    let mut seen_outputs: std::collections::BTreeSet<String> = Default::default();

    for (idx, raw) in code.lines().enumerate() {
        let line_no = (idx as u32) + 1;
        let trimmed = raw.trim();
        let Some(body) = trimmed
            .strip_prefix("//")
            .or_else(|| trimmed.strip_prefix('#'))
            .map(str::trim_start)
        else {
            continue;
        };
        if let Some(rest) = body.strip_prefix("@input") {
            // Frontière stricte : `@inputs`/`@input_x` ne sont pas des
            // directives (le mot doit être suivi d'un blanc ou fin de ligne).
            if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
                continue;
            }
            let rest = rest.trim_start();
            let mut words = rest.splitn(2, char::is_whitespace);
            let name = words.next().unwrap_or("");
            let rest2 = words.next().unwrap_or("").trim();
            if !is_valid_name(name) {
                return Err(DirectiveError {
                    line: line_no,
                    message: format!(
                        "@input « {name} » : nom attendu en snake_case ([a-z_][a-z0-9_]*)"
                    ),
                });
            }
            let mut words2 = rest2.splitn(2, char::is_whitespace);
            let ty_word = words2.next().unwrap_or("");
            let desc_part = words2.next().unwrap_or("");
            let (ty_tok, dflt_text) = match ty_word.split_once('=') {
                Some((t, d)) => (t, Some(d)),
                None => (ty_word, None),
            };
            let ty = if ty_tok.is_empty() {
                return Err(DirectiveError {
                    line: line_no,
                    message: format!("@input {name} : type requis (number|string|bool|any)"),
                });
            } else {
                parse_type(ty_tok, line_no)?
            };
            let default = match dflt_text {
                Some(text) => Some(parse_default(ty, text, line_no)?),
                None => None,
            };
            if !seen_inputs.insert(name.to_string()) {
                return Err(DirectiveError {
                    line: line_no,
                    message: format!("@input {name} : entrée dupliquée"),
                });
            }
            sig.inputs.push(FunctionInput {
                name: name.to_string(),
                ty,
                default,
                desc: clean_desc(desc_part),
            });
        } else if let Some(rest) = body.strip_prefix("@output") {
            if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
                continue;
            }
            let rest = rest.trim_start();
            let mut words = rest.splitn(2, char::is_whitespace);
            let name = words.next().unwrap_or("");
            let rest2 = words.next().unwrap_or("").trim();
            if !is_valid_name(name) {
                return Err(DirectiveError {
                    line: line_no,
                    message: format!(
                        "@output « {name} » : nom attendu en snake_case ([a-z_][a-z0-9_]*)"
                    ),
                });
            }
            let mut words2 = rest2.splitn(2, char::is_whitespace);
            let ty_word = words2.next().unwrap_or("");
            let desc_part = words2.next().unwrap_or("");
            let ty = parse_type(ty_word, line_no)?;
            if !seen_outputs.insert(name.to_string()) {
                return Err(DirectiveError {
                    line: line_no,
                    message: format!("@output {name} : sortie dupliquée"),
                });
            }
            sig.outputs.push(FunctionOutput {
                name: name.to_string(),
                ty,
                desc: clean_desc(desc_part),
            });
        }
        // Tout autre commentaire : ignoré.
    }
    Ok(sig)
}

/// Position d'une directive `@input`/`@output` dans le code (ligne 1-based).
/// Sert aux grilles de ports éditables : la ligne identifie la directive à
/// réécrire quand l'utilisateur modifie une cellule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectiveSpan {
    pub is_input: bool,
    pub name: String,
    pub line: u32,
}

/// Positions de toutes les directives, tolérant aux erreurs et aux
/// doublons (contrairement à [`parse_directives`] qui échoue) : l'éditeur
/// a besoin de localiser les directives même sur un code invalide.
pub fn directive_spans(code: &str) -> Vec<DirectiveSpan> {
    let mut spans = Vec::new();
    for (idx, raw) in code.lines().enumerate() {
        let trimmed = raw.trim();
        let Some(body) = trimmed
            .strip_prefix("//")
            .or_else(|| trimmed.strip_prefix('#'))
            .map(str::trim_start)
        else {
            continue;
        };
        for (word, is_input) in [("@input", true), ("@output", false)] {
            if let Some(rest) = body.strip_prefix(word) {
                // Même frontière stricte que `parse_directives` : le mot
                // doit être suivi d'un blanc ou d'une fin de ligne.
                if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
                    continue;
                }
                if let Some(name) = rest.split_whitespace().next() {
                    spans.push(DirectiveSpan {
                        is_input,
                        name: name.to_string(),
                        line: (idx as u32) + 1,
                    });
                }
                break;
            }
        }
    }
    spans
}

// ───────────────────────── Bindings & chemins msg ─────────────────────────

/// Binding par défaut d'une entrée : `payload.<name>`.
pub fn default_binding(input_name: &str) -> String {
    format!("payload.{input_name}")
}

/// Résout un chemin pointé (`payload.temperature`, segments numériques =
/// index de tableau) dans un msg JSON. `None` = chemin absent.
pub fn resolve_msg_path<'a>(
    msg: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut cur = msg;
    for seg in path.split('.') {
        match cur {
            serde_json::Value::Object(map) => cur = map.get(seg)?,
            serde_json::Value::Array(items) => {
                let idx: usize = seg.parse().ok()?;
                cur = items.get(idx)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

/// Insère une valeur à un chemin pointé du msg (crée les objets
/// intermédiaires ; écrase une valeur existante). Miroir de
/// [`resolve_msg_path`] pour le panneau de test : les valeurs du formulaire
/// typé sont injectées aux chemins de binding avant l'exécution.
pub fn insert_msg_path(msg: &mut serde_json::Value, path: &str, value: serde_json::Value) {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() {
        return;
    }
    let mut cur = msg;
    for (i, seg) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            if !cur.is_object() {
                *cur = serde_json::Value::Object(serde_json::Map::new());
            }
            cur.as_object_mut()
                .expect("objet assuré")
                .insert((*seg).to_string(), value);
            return;
        }
        if !cur.is_object() {
            *cur = serde_json::Value::Object(serde_json::Map::new());
        }
        cur = cur
            .as_object_mut()
            .expect("objet assuré")
            .entry((*seg).to_string())
            .or_insert(serde_json::Value::Object(serde_json::Map::new()));
    }
}

/// Construit l'objet `inputs` passé à `handle`. Routage par fil : si
/// `msg.topic` nomme une entrée déclarée (fil posé sur son ancre — tagger
/// inséré par la projection, ou sortie JSON Split), l'entrée reçoit le
/// **payload entrant** ; sinon elle lit implicitement `payload.<nom>` ;
/// absent → défaut déclaré ; sinon `null` (documenté au cheat-sheet).
pub fn build_inputs(
    inputs: &[FunctionInput],
    msg: &serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    let topic = msg.get("topic").and_then(serde_json::Value::as_str);
    inputs
        .iter()
        .map(|input| {
            let value = if topic == Some(input.name.as_str()) {
                msg.get("payload")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null)
            } else {
                let path = default_binding(&input.name);
                resolve_msg_path(msg, &path)
                    .cloned()
                    .or_else(|| input.default.clone())
                    .unwrap_or(serde_json::Value::Null)
            };
            (input.name.clone(), value)
        })
        .collect()
}

// ───────────────────────── Codegen wrapper JS ─────────────────────────

/// Génère le corps de `async function __el_user_func(msg) { … }` consommé
/// par le nœud `function` du vendor EdgeLinkd (QuickJS) : helpers embarqués
/// + code utilisateur (définissant `handle`) + tail qui construit `inputs`
///   (chaque entrée lit implicitement `payload.<nom>`, défauts déclarés
///   inclus) et mappe le retour en ports — **objet clé→valeur routé par
///   `@output` déclaré** (clé absente = port muet) quand des sorties sont
///   déclarées, contrat historique sinon. Les chaînes sont insérées en
///   littéraux JS échappés (serde_json).
pub fn build_js_wrapper(
    user_code: &str,
    inputs: &[FunctionInput],
    outputs: &[FunctionOutput],
) -> String {
    const HELPERS: &str = r#"// ── PNEX function wrapper (généré) ──
function __pnex_get(msg, path, dflt) {
  let cur = msg;
  for (const seg of String(path).split(".")) {
    if (cur === null || typeof cur !== "object") return dflt;
    cur = cur[seg];
  }
  return cur === undefined || cur === null ? dflt : cur;
}
function __pnex_out(msg, r) {
  if (r === null || r === undefined) return null;
  if (typeof r === "object" && "payload" in r) return Object.assign({}, msg, r);
  return Object.assign({}, msg, { payload: r });
}
function __pnex_port(msg, v) {
  if (v === null || v === undefined) return null;
  return Object.assign({}, msg, { payload: v });
}
"#;

    // Tail généré selon les sorties déclarées : **objet clé→valeur routé
    // par port** (clé absente/null = port muet) ; sinon contrat historique
    // (tableau positionnel, valeur/objet → port 0).
    let tail = if outputs.is_empty() {
        "const __pnex_r = await Promise.resolve(handle(__pnex_inputs, msg));\n\
         if (__pnex_r === null || __pnex_r === undefined) return null;\n\
         if (Array.isArray(__pnex_r)) return __pnex_r.map(function (x) { return __pnex_out(msg, x); });\n\
         return [__pnex_out(msg, __pnex_r)];\n"
            .to_string()
    } else {
        let ports = outputs
            .iter()
            .map(|o| {
                let lit = serde_json::to_string(&o.name).unwrap_or_default();
                format!("__pnex_port(msg, __pnex_r[{lit}])")
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "const __pnex_r = await Promise.resolve(handle(__pnex_inputs, msg));\n\
             if (__pnex_r === null || __pnex_r === undefined) return null;\n\
             if (typeof __pnex_r === \"object\" && !Array.isArray(__pnex_r)) return [{ports}];\n\
             if (Array.isArray(__pnex_r)) return __pnex_r.map(function (x) {{ return __pnex_out(msg, x); }});\n\
             return [__pnex_out(msg, __pnex_r)];\n"
        )
    };

    let mut js = String::with_capacity(user_code.len() + 1024);
    js.push_str(HELPERS);
    js.push('\n');
    js.push_str(user_code.trim_end());
    js.push_str("\n\nconst __pnex_inputs = {\n");
    for input in inputs {
        let path = default_binding(&input.name);
        let name_lit = serde_json::to_string(&input.name).unwrap_or_default();
        let path_lit = serde_json::to_string(&path).unwrap_or_default();
        let dflt_lit = input
            .default
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".into()))
            .unwrap_or_else(|| "undefined".to_string());
        // Routage par fil : `topic` nommant l'entrée → payload entrant.
        js.push_str(&format!(
            "  {name_lit}: (typeof msg.topic === \"string\" && msg.topic === {name_lit}) ? (msg.payload === undefined ? null : msg.payload) : __pnex_get(msg, {path_lit}, {dflt_lit}),\n"
        ));
    }
    js.push_str("};\n");
    js.push_str(&tail);
    js
}

/// Génère le corps du **tagger** inséré par la projection sur chaque fil
/// arrivant sur l'ancre d'une entrée déclarée d'une fonction : estampe
/// `msg.topic` avec le nom de l'entrée — le wrapper JS comme l'exécuteur
/// Starlark routent alors le payload entrant vers `inputs.<nom>` (voir
/// `build_inputs` / `build_js_wrapper`). Le texte est le **corps** de
/// `__el_user_func(msg)` (contrat du nœud vendor `function`) : le `return`
/// doit être au niveau du corps — un `function handle() {}` défini sans
/// être appelé ne retourne rien et le message serait jeté.
pub fn build_input_tag_func(pin: &str) -> String {
    let pin_lit = serde_json::to_string(pin).unwrap_or_else(|_| "\"\"".into());
    format!("return Object.assign({{}}, msg, {{ topic: {pin_lit} }});")
}

// ───────────────── Générateurs JSON Split / JSON Merge ─────────────────

/// Génère le corps du nœud vendor `function` pour un nœud `json-split`.
/// `keys` vide (legacy) : objet → un msg par clé sur l'unique port
/// (`payload` = valeur, `topic` = clé), array → un msg par élément
/// (`topic` = index) ; tout autre payload passe inchangé. `keys` non vide :
/// un port **nommé par clé** — `payload[key]` sort sur son port, clé absente
/// = port muet (slot `null`, contrat vendor), array → éléments sur le port
/// 0, autre payload → passthrough port 0 ; clé sans port = ignorée.
pub fn build_json_split_func(keys: &[String]) -> String {
    if keys.is_empty() {
        return r#"const p = msg.payload;
if (Array.isArray(p)) {
  return [p.map(function (v, i) {
    return Object.assign({}, msg, { payload: v, topic: String(i) });
  })];
}
if (p !== null && typeof p === "object") {
  return [Object.keys(p).map(function (k) {
    return Object.assign({}, msg, { payload: p[k], topic: k });
  })];
}
return [msg];"#
            .to_string();
    }
    let keys_lit = serde_json::to_string(keys).unwrap_or_else(|_| "[]".into());
    format!(
        r#"var keys = {keys_lit};
var p = msg.payload;
if (Array.isArray(p)) {{
  return [p.map(function (v, i) {{
    return Object.assign({{}}, msg, {{ payload: v, topic: String(i) }});
  }})];
}}
if (p !== null && typeof p === "object") {{
  var ports = [];
  for (var i = 0; i < keys.length; i++) {{
    var k = keys[i];
    if (Object.prototype.hasOwnProperty.call(p, k)) {{
      ports.push([Object.assign({{}}, msg, {{ payload: p[k], topic: k }})]);
    }} else {{
      ports.push(null);
    }}
  }}
  return ports;
}}
return [msg];"#
    )
}

/// Génère le corps du nœud vendor `function` pour un nœud `json-merge` :
/// chaque payload est rangé sous `msg.topic` (si non vide) sinon sous
/// `default_key` dans un accumulateur persistant (contexte du nœud) — une
/// entrée = une variable ; l'objet accumulé est émis à chaque msg. Style
/// volontairement `var`/`Object.assign` (pas de spread/arrow) — école du
/// wrapper de fonction.
pub fn build_json_merge_func(default_key: &str) -> String {
    let key_lit = serde_json::to_string(default_key).unwrap_or_else(|_| "\"value\"".into());
    format!(
        r#"var key = (typeof msg.topic === "string" && msg.topic) ? msg.topic : {key_lit};
var acc = context.get("__pnex_merge");
if (acc === null || typeof acc !== "object" || Array.isArray(acc)) acc = {{}};
acc = Object.assign({{}}, acc);
acc[key] = msg.payload;
context.set("__pnex_merge", acc);
return [Object.assign({{}}, msg, {{ payload: acc }})];"#
    )
}

// ───────────────────── Config du nœud de flow + snapshots ─────────────────────

/// Configuration du nœud custom PNEX `PnexFunction` — **références +
/// snapshot d'interface** uniquement : le CODE n'est jamais dans le graphe,
/// la projection l'inline au deploy depuis la version épinglée (artefact
/// self-contained ; éditer une fonction n'affecte pas les flows déployés
/// tant qu'ils ne sont pas re-déployés).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FunctionNodeConfig {
    /// `functions.id` — 0 = non sélectionné (validate_graph : `fn_not_selected`).
    #[serde(default)]
    pub function_id: i64,
    /// Nom conservé pour l'affichage (le picker le remplit).
    #[serde(default)]
    pub function_name: String,
    /// `function_versions.version_number` épinglée — 0 = non posé.
    #[serde(default)]
    pub version_number: i64,
    #[serde(default)]
    pub language: FunctionLanguage,
    /// Snapshot des entrées/sorties déclarées (directives de la version
    /// épinglée) — pilote l'inspecteur et le panneau de test.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<FunctionInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<FunctionOutput>,
}

/// Instantané d'une version de fonction résolue au deploy (backend) —
/// porte le CODE qui sera inline dans l'artefact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionSnapshot {
    pub function_id: i64,
    pub version_number: i64,
    pub language: FunctionLanguage,
    pub code: String,
    #[serde(default)]
    pub inputs: Vec<FunctionInput>,
    #[serde(default)]
    pub outputs: Vec<FunctionOutput>,
}

/// Résolveur passé à la projection : `(function_id, version_number) →
/// snapshot`. Le backend le construit en batch depuis `function_versions`.
pub type FunctionResolver = BTreeMap<(i64, i64), FunctionSnapshot>;

// ───────────────────────────── DTOs de l'API ─────────────────────────────

/// Résumé d'une fonction (liste paginée — sans code).
/// `PartialEq` requis par les props des composants du socle CRUD (macro
/// `#[component]` dioxus 0.7 — impl généré par champ).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionSummary {
    pub id: i64,
    pub org_id: i64,
    pub name: String,
    pub language: FunctionLanguage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub current_version_number: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Fonction + version courante (code + interface).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDetail {
    pub id: i64,
    pub org_id: i64,
    pub name: String,
    pub language: FunctionLanguage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub current_version_number: i64,
    pub code: String,
    #[serde(default)]
    pub inputs: Vec<FunctionInput>,
    #[serde(default)]
    pub outputs: Vec<FunctionOutput>,
    pub created_at: String,
    pub updated_at: String,
}

/// Résumé d'une version (drawer d'historique + **picker de pin** de
/// l'inspecteur : l'inspecteur pique le snapshot d'interface ici — plus
/// besoin d'un endpoint par version).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionVersionSummary {
    pub id: i64,
    pub version_number: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<FunctionInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<FunctionOutput>,
    pub created_at: String,
    /// Full code of this version — the history drawer restores ANY past
    /// version into the editor (append-only: saving then creates v+1).
    pub code: String,
}

/// Création : fonction + version 1 (école `CreateFlow` — une transaction).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFunction {
    pub name: String,
    pub language: FunctionLanguage,
    #[serde(default)]
    pub description: Option<String>,
    pub code: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Enregistrement d'une nouvelle version (append-only). Concurrence
/// optimiste : `expected_version_number` doit valoir la version courante,
/// sinon 409. `name`/`description` = mise à jour de métadonnées ; avec
/// `code`, le save crée une nouvelle version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveFunctionVersion {
    pub expected_version_number: i64,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Test ad-hoc d'un code non sauvegardé (test avant save).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionTestAdHoc {
    pub language: FunctionLanguage,
    pub code: String,
}

/// Demande de test en live — version épinglée de la fonction (défaut =
/// version courante) OU code ad-hoc. `inputs` = valeurs du formulaire typé
/// ; `msg` = message Node-RED entrant simulé (défaut `{}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionTestRequest {
    #[serde(default)]
    pub version_number: Option<i64>,
    #[serde(default)]
    pub ad_hoc: Option<FunctionTestAdHoc>,
    #[serde(default)]
    pub inputs: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub msg: Option<serde_json::Value>,
}

/// Réponse du test en live (aussi la ligne unique du CLI
/// `pnex-flow-runtime --test-function`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionTestResponse {
    pub ok: bool,
    /// Messages sortants (un par port ayant produit un msg, dans l'ordre).
    pub outputs: Vec<serde_json::Value>,
    /// Sortie `console.*` capturée (mode test).
    pub logs: Vec<String>,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Requête portée au CLI `pnex-flow-runtime --test-function` — le wrapper
/// JS est **déjà généré** par le backend (`build_js_wrapper`), le CLI ne
/// dépend pas de pnex-core à l'exécution (types seulement).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionExecRequest {
    pub language: FunctionLanguage,
    /// Wrapper JS complet (helpers + code + tail) — requis si `js`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub js_wrapper: Option<String>,
    /// Code Starlark brut — requis si `starlark`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default)]
    pub inputs: Vec<FunctionInput>,
    pub msg: serde_json::Value,
    pub timeout_ms: u64,
}

/// Requête de validation compile-only (barre d'erreurs de l'éditeur) —
/// `POST /api/v1/functions/validate`, retransmise telle quelle au CLI
/// `pnex-flow-runtime --check-function`. Aucune exécution du code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionValidateRequest {
    pub language: FunctionLanguage,
    pub code: String,
}

/// Diagnostic d'éditeur — ligne/colonne 1-based, best-effort (None quand le
/// moteur ne les expose pas). Message verbatim du moteur (exception
/// documentée : diagnostics starlark/quickjs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDiagnostic {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col: Option<u32>,
    pub message: String,
}

/// Réponse de la validation compile-only (aussi la ligne unique du CLI
/// `pnex-flow-runtime --check-function`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionValidateResponse {
    pub ok: bool,
    pub diagnostics: Vec<FunctionDiagnostic>,
}

// ──────────────────────────────── Tests ────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_directives_complet() {
        let code = r#"
// @input temperature number "Température mesurée"
// @input mode string="auto" Mode de marche
// @input enabled bool=true
# @output heating number
# @output alarm bool "Alarme process"
function handle(inputs, msg) { return inputs.temperature; }
"#;
        let sig = parse_directives(code).unwrap();
        assert_eq!(sig.inputs.len(), 3);
        assert_eq!(sig.outputs.len(), 2);
        assert_eq!(sig.inputs[0].name, "temperature");
        assert_eq!(sig.inputs[0].ty, FunctionType::Number);
        assert_eq!(sig.inputs[0].desc.as_deref(), Some("Température mesurée"));
        assert_eq!(sig.inputs[1].ty, FunctionType::String);
        assert_eq!(sig.inputs[1].default, Some(json!("auto")));
        assert_eq!(sig.inputs[1].desc.as_deref(), Some("Mode de marche"));
        assert_eq!(sig.inputs[2].default, Some(json!(true)));
        assert_eq!(sig.outputs[0].name, "heating");
        assert_eq!(sig.outputs[1].desc.as_deref(), Some("Alarme process"));
    }

    #[test]
    fn parse_directives_vide_et_sans_directive() {
        assert!(parse_directives("").unwrap().inputs.is_empty());
        assert!(parse_directives("// rien ici\nlet x = 1;")
            .unwrap()
            .outputs
            .is_empty());
    }

    #[test]
    fn parse_directives_erreurs() {
        // type inconnu
        let e = parse_directives("// @input a float").unwrap_err();
        assert_eq!(e.line, 1);
        assert!(e.message.contains("type inconnu"), "{}", e.message);
        // défaut non convertible
        let e = parse_directives("// @input a number=abc").unwrap_err();
        assert!(e.message.contains("nombre attendu"), "{}", e.message);
        // dupliquée
        let e = parse_directives("// @input a number\n// @input a string").unwrap_err();
        assert!(e.message.contains("dupliquée"), "{}", e.message);
        // nom invalide
        let e = parse_directives("# @input 1abc number").unwrap_err();
        assert!(e.message.contains("snake_case"), "{}", e.message);
        // type manquant
        let e = parse_directives("// @input a").unwrap_err();
        assert!(e.message.contains("type requis"), "{}", e.message);
    }

    #[test]
    fn parse_directives_defaut_any_json() {
        let sig = parse_directives("// @input cfg any={\"limit\":5}").unwrap();
        assert_eq!(sig.inputs[0].default, Some(json!({"limit": 5})));
    }

    #[test]
    fn directive_spans_positions() {
        let code = "def handle(inputs, msg):\n\
                    # @input temperature number\n\
                    return inputs[\"temperature\"]\n\
                    // @output heating number\n\
                    // @output delta any\n\
                    // not a directive\n\
                    # @inputs nope";
        let spans = directive_spans(code);
        assert_eq!(
            spans,
            vec![
                DirectiveSpan {
                    is_input: true,
                    name: "temperature".into(),
                    line: 2
                },
                DirectiveSpan {
                    is_input: false,
                    name: "heating".into(),
                    line: 4
                },
                DirectiveSpan {
                    is_input: false,
                    name: "delta".into(),
                    line: 5
                },
            ]
        );
    }

    #[test]
    fn directive_spans_tolerant() {
        // Invalid directives still yield spans (the editor needs positions
        // even on broken code); `@inputs` strict boundary is skipped.
        let spans = directive_spans("// @input temp\n// @input\n// @inputs x number");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].line, 1);
        assert_eq!(spans[0].name, "temp");
        assert!(directive_spans("").is_empty());
        assert!(directive_spans("x = 1").is_empty());
    }

    #[test]
    fn resolve_msg_path_chemins() {
        let msg = json!({"payload": {"t": [10, 20]}, "topic": "a"});
        assert_eq!(resolve_msg_path(&msg, "payload.t.1"), Some(&json!(20)));
        assert_eq!(resolve_msg_path(&msg, "topic"), Some(&json!("a")));
        assert_eq!(resolve_msg_path(&msg, "payload.t.9"), None);
        assert_eq!(resolve_msg_path(&msg, "payload.x.y"), None);
    }

    #[test]
    fn insert_msg_path_cree_les_niveaux() {
        let mut msg = serde_json::json!({"topic": "a"});
        insert_msg_path(&mut msg, "payload.temperature", serde_json::json!(10));
        assert_eq!(msg["payload"]["temperature"], 10);
        assert_eq!(msg["topic"], "a");
        // Écrase une valeur existante.
        insert_msg_path(&mut msg, "payload.temperature", serde_json::json!(20));
        assert_eq!(msg["payload"]["temperature"], 20);
        // Chemin racine traversant un non-objet : remplacé par un objet.
        let mut scalar = serde_json::json!(42);
        insert_msg_path(&mut scalar, "payload.x", serde_json::json!(1));
        assert_eq!(scalar["payload"]["x"], 1);
        // Roundtrip avec resolve_msg_path.
        assert_eq!(
            resolve_msg_path(&msg, "payload.temperature"),
            Some(&serde_json::json!(20))
        );
    }

    #[test]
    fn build_inputs_default_payload_name() {
        let inputs = vec![
            FunctionInput {
                name: "t".into(),
                ty: FunctionType::Number,
                default: Some(json!(0)),
                desc: None,
            },
            FunctionInput {
                name: "mode".into(),
                ty: FunctionType::String,
                default: None,
                desc: None,
            },
        ];
        // Chaque entrée lit implicitement payload.<nom> : t ← payload.t
        // (absent → défaut déclaré 0) ; mode ← payload.mode (absent, pas de
        // défaut → null).
        let inputs_obj = build_inputs(&inputs, &json!({}));
        assert_eq!(inputs_obj.get("t"), Some(&json!(0)));
        assert_eq!(
            inputs_obj.get("mode"),
            Some(&json!(serde_json::Value::Null))
        );
        let inputs_obj = build_inputs(&inputs, &json!({"payload": {"t": 42}}));
        assert_eq!(inputs_obj.get("t"), Some(&json!(42)));
    }

    #[test]
    fn build_inputs_topic_route_le_payload() {
        let inputs = vec![FunctionInput {
            name: "t".into(),
            ty: FunctionType::Number,
            default: Some(json!(0)),
            desc: None,
        }];
        // msg.topic nommant l'entrée → payload entrant (routage par fil),
        // même si payload.<nom> existerait par ailleurs.
        let inputs_obj = build_inputs(&inputs, &json!({"topic": "t", "payload": 42}));
        assert_eq!(inputs_obj.get("t"), Some(&json!(42)));
        // Topic inconnu → lecture implicite payload.<nom> (fallback).
        let inputs_obj = build_inputs(&inputs, &json!({"topic": "autre", "payload": {"t": 7}}));
        assert_eq!(inputs_obj.get("t"), Some(&json!(7)));
        // Topic nommant l'entrée mais payload absent → null.
        let inputs_obj = build_inputs(&inputs, &json!({"topic": "t"}));
        assert_eq!(inputs_obj.get("t"), Some(&json!(serde_json::Value::Null)));
    }

    #[test]
    fn build_input_tag_func_estampe_le_topic() {
        let js = build_input_tag_func("in_float");
        // Corps direct de __el_user_func : le return au niveau du corps
        // (un function handle défini sans être appelé → message jeté).
        assert!(
            js.starts_with("return Object.assign"),
            "corps direct, return top-level"
        );
        assert!(
            js.contains("topic: \"in_float\""),
            "le tagger estampe topic avec le nom de l'entrée"
        );
        assert!(
            !js.contains("function handle"),
            "jamais de déclaration handle non appelée"
        );
    }

    #[test]
    fn build_js_wrapper_structure() {
        let inputs = vec![FunctionInput {
            name: "t".into(),
            ty: FunctionType::Number,
            default: Some(json!(21.5)),
            desc: None,
        }];
        let code = "function handle(inputs, msg) { return inputs.t + 1; }";
        let js = build_js_wrapper(code, &inputs, &[]);
        assert!(js.contains("__pnex_get"), "helpers absents");
        assert!(js.contains("function handle(inputs, msg)"), "code user");
        assert!(
            js.contains("\"payload.t\""),
            "chemin implicite payload.<nom>"
        );
        assert!(js.contains("21.5"), "défaut sérialisé");
        assert!(js.contains("__pnex_inputs"), "objet inputs");
        assert!(
            js.contains("msg.topic === \"t\""),
            "branche de routage par topic"
        );
        assert!(js.contains("await Promise.resolve(handle("), "tail");
        // Échappement : un nom d'entrée contenant une quote ne casse pas le JS.
        let evil = vec![FunctionInput {
            name: "a\"b".into(),
            ty: FunctionType::Number,
            default: None,
            desc: None,
        }];
        let js = build_js_wrapper(code, &evil, &[]);
        assert!(
            js.contains("\"payload.a\\\"b\""),
            "échappement JSON du chemin"
        );
    }

    #[test]
    fn build_js_wrapper_route_le_retour_par_sorties() {
        let fout = |name: &str| FunctionOutput {
            name: name.into(),
            ty: FunctionType::Bool,
            desc: None,
        };
        let code = "function handle(inputs, msg) { return {out_bool: true}; }";
        let js = build_js_wrapper(code, &[], &[fout("out_bool"), fout("out_temp")]);
        // Objet clé→valeur routé par port, dans l'ordre des @output.
        assert!(
            js.contains(
                "return [__pnex_port(msg, __pnex_r[\"out_bool\"]), __pnex_port(msg, __pnex_r[\"out_temp\"])];"
            ),
            "routage clé→port généré"
        );
        // Sans sorties déclarées : contrat historique (port 0).
        let js = build_js_wrapper(code, &[], &[]);
        assert!(
            !js.contains("__pnex_port(msg, __pnex_r["),
            "pas de routage sans @output"
        );
        assert!(
            js.contains("return [__pnex_out(msg, __pnex_r)];"),
            "retour historique port 0"
        );
    }

    #[test]
    fn json_split_genere_fan_out_par_cle() {
        let js = build_json_split_func(&[]);
        assert!(
            js.contains("Array.isArray(p)"),
            "branche array (fan-out par élément)"
        );
        assert!(js.contains("Object.keys(p).map"), "fan-out par clé (objet)");
        assert!(js.contains("topic: k"), "topic = clé");
        assert!(
            js.contains("return [msg]"),
            "pass-through des payloads ni objet ni array"
        );
        // Aucun littéral de clés : contrat legacy 1 port conservé verbatim.
        assert!(!js.contains("var keys"), "legacy sans clés déclarées");
    }

    #[test]
    fn json_split_genere_ports_nommes() {
        let js = build_json_split_func(&["a".to_string(), "b".to_string()]);
        assert!(
            js.contains(r#"var keys = ["a","b"]"#),
            "littéral de clés inline (serde_json) : {js}"
        );
        assert!(
            js.contains("hasOwnProperty.call(p, k)"),
            "slot seulement pour les clés présentes"
        );
        assert!(
            js.contains("ports.push(null)"),
            "port muet (slot null) pour clé absente"
        );
        assert!(
            js.contains("Array.isArray(p)"),
            "array → port 0 (tous les éléments)"
        );
        assert!(
            js.contains("return [msg]"),
            "passthrough port 0 des autres payloads"
        );
    }

    #[test]
    fn json_merge_genere_accumulation_contexte() {
        let js = build_json_merge_func("ma clé");
        assert!(
            js.contains("context.get(\"__pnex_merge\")"),
            "lecture accumulateur"
        );
        assert!(
            js.contains("context.set(\"__pnex_merge\", acc)"),
            "écriture accumulateur"
        );
        assert!(
            js.contains("acc[key] = msg.payload"),
            "une entrée = une variable (plus de shallow-merge objet)"
        );
        assert!(
            !js.contains("Object.assign({}, acc, msg.payload)"),
            "branche shallow-merge objet supprimée"
        );
        assert!(
            js.contains("\"ma clé\""),
            "clé par défaut inline échappée (serde_json)"
        );
    }

    #[test]
    fn language_output_count_min_1() {
        assert_eq!(FunctionLanguage::output_count(&[]), 1);
    }

    #[test]
    fn dto_roundtrip() {
        let detail = FunctionDetail {
            id: 3,
            org_id: 7,
            name: "chauffage".into(),
            language: FunctionLanguage::Starlark,
            description: Some("d".into()),
            current_version_number: 2,
            code: "def handle(inputs, msg):".into(),
            inputs: vec![],
            outputs: vec![],
            created_at: "2026-09-15T00:00:00Z".into(),
            updated_at: "2026-09-15T00:00:00Z".into(),
        };
        let back: FunctionDetail =
            serde_json::from_str(&serde_json::to_string(&detail).unwrap()).unwrap();
        assert_eq!(back.language, FunctionLanguage::Starlark);
        assert_eq!(back.current_version_number, 2);
    }
}
