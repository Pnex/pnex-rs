//! Rendu minijinja des templates (D52) — partagé backend (preview API,
//! boutons Test) et nœud flow (rendu au send).
//!
//! Contexte (deux niveaux) :
//! 1. les `vars` du nœud sont rendues **d'abord** contre `{msg, meta}` —
//!    une var peut être un littéral `"80"` ou une référence `"{{ msg.x }}"`
//!    (expression injinja-invalide = littéral, pas d'erreur) ;
//! 2. le template est rendu contre `{msg, meta, vars}`.
//!
//! Le payload entrant est **aussi** exposé à la racine (`{{ temperature }}`
//! = `{{ msg.temperature }}`) pour l'ergonomie ; en collision, la clé
//! explicite (`msg`/`meta`/`vars`) gagne car insérée après.
//! Les `vars` rendues suivent la même école : référençables sans préfixe
//! (`{{ seuil }}` = `{{ vars.seuil }}`) — précédence : clé payload < var
//! racine < namespace explicite.
//!
//! minijinja est sandbox par conception (pas d'accès disque, pas d'include
//! arbitraire) ; la seule borne à poser soi-même est la longueur de sortie.

use std::collections::BTreeMap;

use minijinja::Value;

use crate::error::NotifyError;
use crate::Message;

/// Borne anti auto-inondation : un template utilisateur peut se démultiplier
/// (boucle + concat) — sortie totale (sujet + corps) plafonnée.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Rend le sujet (optionnel) et le corps d'un message depuis un template
/// minijinja. `vars` = overrides du nœud (2 niveaux, cf. doc du module).
pub fn render(
    subject_tpl: Option<&str>,
    body_tpl: &str,
    vars: &BTreeMap<String, String>,
    msg: serde_json::Value,
    meta: serde_json::Value,
) -> Result<Message, NotifyError> {
    // Niveau 1 : les vars contre {msg, meta}. Une var qui n'est pas du
    // Jinja valide est un littéral — jamais une erreur de rendu.
    let mut rendered_vars = BTreeMap::new();
    let inner = root_context(&msg, &meta, None);
    for (k, v) in vars {
        rendered_vars.insert(k.clone(), render_lenient(v, &inner));
    }

    // Niveau 2 : le template contre {msg, meta, vars rendues}.
    let full = root_context(&msg, &meta, Some(rendered_vars.clone()));
    let subject = subject_tpl
        .map(|tpl| render_lenient_checked(tpl, &full))
        .transpose()?;
    let body = render_lenient_checked(body_tpl, &full)?;

    let total = subject.as_deref().map_or(0, str::len) + body.len();
    if total > MAX_OUTPUT_BYTES {
        return Err(NotifyError::TooLarge(total));
    }
    Ok(Message {
        subject,
        body,
        meta,
    })
}

/// Detects the template variables (subject first, then body), deduplicated
/// in order of first occurrence — this order becomes the order of the
/// notify node's canvas input anchors.
///
/// Extraction relies on minijinja's AST (`Template::undeclared_variables`),
/// so any expression declares its variables: `{{ x }}`, `{{ x | round }}`,
/// `{{ (x / 60) | int }}`, `{{ x if y else z }}`, `{% if x %}`, and the
/// `vars.<leaf>` form (`{{ vars.x | upper }}` → `x`).
///
/// Ignored: the reserved namespaces (`msg`, `meta`, `vars` and their
/// attributes), names assigned inside the template (`{% set %}`, loop
/// variables), minijinja globals/specials (`range`, `loop`, …), non
/// snake_case names, and templates that do not parse (rendering reports
/// the syntax error).
pub fn template_vars(subject: Option<&str>, body: &str) -> Vec<String> {
    let mut vars = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(subject) = subject {
        scan_template_vars(subject, &mut vars, &mut seen);
    }
    scan_template_vars(body, &mut vars, &mut seen);
    vars
}

/// Names minijinja resolves by itself (specials, not context lookups).
const TEMPLATE_SPECIALS: &[&str] = &["loop", "self", "super", "caller", "varargs", "kwargs"];

fn scan_template_vars(
    tpl: &str,
    vars: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) {
    let declared = declared_template_vars(tpl);
    if declared.is_empty() {
        return;
    }
    // `undeclared_variables` returns an unordered set: order by the first
    // occurrence of the name inside a template tag.
    for name in ordered_tag_idents(tpl) {
        if declared.contains(&name) && seen.insert(name.clone()) {
            vars.push(name);
        }
    }
}

/// Variables a template reads from its context, as assignable var names:
/// top-level names (snake_case, not a namespace/global/special) plus the
/// leaves of `vars.<leaf>`. Empty when the template does not parse.
fn declared_template_vars(tpl: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let mut env = minijinja::Environment::new();
    if env.add_template("t", tpl).is_err() {
        return out;
    }
    let Ok(tmpl) = env.get_template("t") else {
        return out;
    };
    let globals: std::collections::HashSet<&str> = env.globals().map(|(k, _)| k).collect();
    for name in tmpl.undeclared_variables(false) {
        if is_snake_ident(&name)
            && !matches!(name.as_str(), "msg" | "meta" | "vars")
            && !globals.contains(name.as_str())
            && !TEMPLATE_SPECIALS.contains(&name.as_str())
        {
            out.insert(name);
        }
    }
    for path in tmpl.undeclared_variables(true) {
        if let Some(leaf) = path.strip_prefix("vars.") {
            // `vars.a.b` → leaf `a` (the var itself is what gets assigned).
            let leaf = leaf.split('.').next().unwrap_or(leaf);
            if is_snake_ident(leaf) {
                out.insert(leaf.to_string());
            }
        }
    }
    out
}

/// Identifiers found inside `{{ }}` / `{% %}` tags, in source order: a
/// top-level name as is, the attribute following `vars.` as its leaf name;
/// other attributes (`msg.x`, `a.b`) and string literals are skipped.
/// Used only to order the set computed from the AST.
fn ordered_tag_idents(tpl: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = tpl;
    loop {
        let open = match (rest.find("{{"), rest.find("{%")) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => break,
        };
        let close_tag = if rest[open..].starts_with("{{") {
            "}}"
        } else {
            "%}"
        };
        let inner_start = open + 2;
        let Some(len) = rest[inner_start..].find(close_tag) else {
            break;
        };
        tag_idents(&rest[inner_start..inner_start + len], &mut out);
        rest = &rest[inner_start + len + 2..];
    }
    out
}

fn tag_idents(expr: &str, out: &mut Vec<String>) {
    let bytes = expr.as_bytes();
    let mut i = 0;
    // Previous identifier and whether it was followed by a `.`.
    let mut prev_ident: Option<&str> = None;
    let mut after_dot = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\'' || c == b'"' {
            // Skip a string literal (escapes included).
            let quote = c;
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
            prev_ident = None;
            after_dot = false;
        } else if c == b'_' || c.is_ascii_alphabetic() {
            let start = i;
            while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            let ident = &expr[start..i];
            if !after_dot {
                out.push(ident.to_string());
            } else if prev_ident == Some("vars") {
                out.push(ident.to_string());
            }
            prev_ident = if after_dot { None } else { Some(ident) };
            after_dot = false;
        } else if c == b'.' {
            after_dot = true;
            i += 1;
        } else {
            if !c.is_ascii_whitespace() {
                prev_ident = None;
                after_dot = false;
            }
            i += 1;
        }
    }
}

/// snake_case `[a-z_][a-z0-9_]*` — même école que les directives de
/// fonction `@input`/`@output` (pnex-core/src/functions.rs).
fn is_snake_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some('_') | Some('a'..='z') => {}
        _ => return false,
    }
    chars.all(|c| matches!(c, '_' | 'a'..='z' | '0'..='9'))
}

/// Contexte racine : clés du payload exposées à la racine (si objet), puis
/// `vars` rendues sans préfixe, puis `msg`/`meta`/`vars` (toujours) —
/// l'ordre d'insertion fait la précédence : payload < var racine <
/// namespace explicite.
fn root_context(
    msg: &serde_json::Value,
    meta: &serde_json::Value,
    vars: Option<BTreeMap<String, String>>,
) -> serde_json::Value {
    let mut root = serde_json::Map::new();
    if let serde_json::Value::Object(entries) = msg {
        for (k, v) in entries {
            root.insert(k.clone(), v.clone());
        }
    }
    if let Some(vars) = &vars {
        // Vars référençables sans préfixe (`{{ seuil }}` = `{{ vars.seuil }}`,
        // même ergonomie que le payload) — insérées après le payload : la var
        // explicite gagne ; insérées avant les namespaces : une var nommée
        // `msg`/`meta`/`vars` ne masque jamais le namespace.
        for (k, v) in vars {
            root.insert(k.clone(), typed_var(v));
        }
    }
    root.insert("msg".into(), msg.clone());
    root.insert("meta".into(), meta.clone());
    if let Some(vars) = vars {
        root.insert(
            "vars".into(),
            serde_json::Value::Object(
                vars.into_iter()
                    .map(|(k, v)| {
                        let typed = typed_var(&v);
                        (k, typed)
                    })
                    .collect(),
            ),
        );
    }
    serde_json::Value::Object(root)
}

/// Rendered vars are strings (declared examples, node overrides, level-1
/// renders). A string that is the exact rendering of a number becomes that
/// typed number, so arithmetic and numeric filters work on
/// declared examples (`{{ (eta / 60) | int }}` with example `"120"`)
/// while `{{ eta }}` still renders byte-identical. Anything else (`"007"`,
/// `"1e3"`, text) stays a string.
fn typed_var(raw: &str) -> serde_json::Value {
    let fallback = || serde_json::Value::String(raw.to_string());
    let candidate = if let Ok(i) = raw.parse::<i64>() {
        serde_json::Value::from(i)
    } else if let Ok(f) = raw.parse::<f64>() {
        match serde_json::Number::from_f64(f) {
            Some(n) => serde_json::Value::Number(n),
            None => return fallback(),
        }
    } else {
        return fallback();
    };
    // Keep the typed value only when it renders exactly like the source.
    if json_to_minijinja(&candidate).to_string() == raw {
        candidate
    } else {
        fallback()
    }
}

/// Converts a `serde_json::Value` into a minijinja value WITHOUT the serde
/// serializer bridge: with `serde_json/arbitrary_precision` anywhere in the
/// dependency graph (feature unification via the flow engine), `Number`
/// serializes to a magic one-entry map for any non-serde_json serializer —
/// rendering `{{ x }}` as `{"$serde_json::private::Number":"10"}` instead
/// of `10`. The manual walk keeps numbers, bools, strings, arrays and
/// objects as first-class template values.
fn json_to_minijinja(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::from(()),
        serde_json::Value::Bool(b) => Value::from(*b),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::from)
            .or_else(|| n.as_u64().map(Value::from))
            .or_else(|| n.as_f64().map(Value::from))
            .unwrap_or_else(|| Value::from(n.to_string())),
        serde_json::Value::String(s) => Value::from(s.as_str()),
        serde_json::Value::Array(items) => {
            Value::from(items.iter().map(json_to_minijinja).collect::<Vec<_>>())
        }
        serde_json::Value::Object(entries) => {
            let mut map = std::collections::BTreeMap::new();
            for (k, v) in entries {
                map.insert(k.clone(), json_to_minijinja(v));
            }
            Value::from(map)
        }
    }
}

/// Rendu indulgent : expression injinja-invalide = littéral retourné tel
/// quel (ergonomie « var = littéral OU référence »).
fn render_lenient(tpl: &str, ctx: &serde_json::Value) -> String {
    render_str(tpl, ctx).unwrap_or_else(|_| tpl.to_string())
}

/// Rendu strict : les erreurs de syntaxe du template principal remontent.
fn render_str(tpl: &str, ctx: &serde_json::Value) -> Result<String, NotifyError> {
    let mut env = minijinja::Environment::new();
    env.add_template("t", tpl)
        .map_err(|e| NotifyError::Render(e.to_string()))?;
    let tmpl = env
        .get_template("t")
        .map_err(|e| NotifyError::Render(e.to_string()))?;
    tmpl.render(json_to_minijinja(ctx))
        .map_err(|e| NotifyError::Render(e.to_string()))
}

fn render_lenient_checked(tpl: &str, ctx: &serde_json::Value) -> Result<String, NotifyError> {
    render_str(tpl, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn payload_expose_a_la_racine() {
        let m = render(
            None,
            "⚠️ {{ device }} : {{ value }} {{ unit }}",
            &vars(&[]),
            json!({"device": "chaudiere", "value": 82.5, "unit": "°C"}),
            json!({"ts": "2026-09-14T10:00:00Z"}),
        )
        .unwrap();
        assert_eq!(m.body, "⚠️ chaudiere : 82.5 °C");
    }

    #[test]
    fn vars_deux_niveaux_litteral_ou_reference() {
        // "80" = littéral ; "{{ msg.value }}" = référence pré-rendue.
        let m = render(
            None,
            "seuil {{ vars.seuil }} vs mesure {{ vars.mesure }}",
            &vars(&[("seuil", "80"), ("mesure", "{{ msg.value }}")]),
            json!({"value": 82.5}),
            json!({}),
        )
        .unwrap();
        assert_eq!(m.body, "seuil 80 vs mesure 82.5");
    }

    #[test]
    fn collision_cle_explicite_gagne() {
        // Un payload portant une clé "meta" : le contexte explicite gagne.
        let m = render(
            None,
            "{{ meta.ts }}",
            &vars(&[]),
            json!({"meta": "payload_piège"}),
            json!({"ts": "T1"}),
        )
        .unwrap();
        assert_eq!(m.body, "T1");
    }

    #[test]
    fn var_explicite_gagne_sur_payload_racine() {
        let m = render(
            None,
            "{{ value }}",
            &vars(&[]),
            json!({"value": "payload"}),
            json!({}),
        )
        .unwrap();
        assert_eq!(m.body, "payload");
        let m2 = render(
            None,
            "{{ vars.value }}",
            &vars(&[("value", "override")]),
            json!({"value": "payload"}),
            json!({}),
        )
        .unwrap();
        assert_eq!(m2.body, "override");
    }

    #[test]
    fn payload_non_objet_access_via_msg() {
        let m = render(
            None,
            "v={{ msg }}",
            &vars(&[]),
            serde_json::json!(42),
            json!({}),
        )
        .unwrap();
        assert_eq!(m.body, "v=42");
    }

    #[test]
    fn var_rendue_exposee_a_la_racine() {
        // Régression UI 2026-09-14 : l'éditeur de templates laisse écrire
        // `{{ test }}` sans préfixe — une var déclarée doit résoudre seul.
        let m = render(
            None,
            "salut {{ test }}",
            &vars(&[("test", "monde")]),
            json!({}),
            json!({}),
        )
        .unwrap();
        assert_eq!(m.body, "salut monde");
    }

    #[test]
    fn precedence_payload_puis_var_puis_namespace() {
        let m = render(
            None,
            "{{ value }}|{{ vars.value }}|{{ meta.ts }}",
            &vars(&[("value", "var"), ("meta", "var_piège")]),
            json!({"value": "payload"}),
            json!({"ts": "T1"}),
        )
        .unwrap();
        // var racine > clé payload ; namespace `vars` > var racine ; une var
        // nommée `meta` ne masque pas le namespace `meta`.
        assert_eq!(m.body, "var|var|T1");
    }

    #[test]
    fn sujet_rendu_si_present() {
        let m = render(
            Some("[{{ meta.org }}] {{ device }}"),
            "corps",
            &vars(&[]),
            json!({"device": "d1"}),
            json!({"org": "Ranch"}),
        )
        .unwrap();
        assert_eq!(m.subject.as_deref(), Some("[Ranch] d1"));
    }

    #[test]
    fn syntaxe_template_invalide_erreur() {
        let e = render(None, "{% if %}", &vars(&[]), json!({}), json!({})).unwrap_err();
        assert!(matches!(e, NotifyError::Render(_)));
    }

    #[test]
    fn borne_64kib() {
        let gros = "x".repeat(70_000);
        let e = render(None, &gros, &vars(&[]), json!({}), json!({})).unwrap_err();
        assert!(matches!(e, NotifyError::TooLarge(70_000)));
    }

    #[test]
    fn vars_detectees_nues_et_vars_leaf_dans_lordre() {
        let out = template_vars(
            Some("[{{ device }}] {{ vars.seuil }}"),
            "v={{ value }} seuil={{ vars.seuil }} 2e={{ second }}",
        );
        // Dédup première occurrence : seuil vu au sujet n'apparaît qu'une fois.
        assert_eq!(out, vec!["device", "seuil", "value", "second"]);
    }

    #[test]
    fn vars_skip_namespaces_but_keep_expression_operands() {
        let out = template_vars(
            None,
            "{{ msg.value }} {{ meta.ts }} {{ vars }} {{ msg }} {{ meta }} \
             {{ value | upper }} {{ a + b }} {{ loop.index }} {% if x %}{{ ok }}{% endif %} \
             {{- trim_both -}} OK_{{ Upper }}",
        );
        // Filter/operator operands and `if` tests are inputs too; namespaces,
        // the `loop` special and non snake_case names are not.
        assert_eq!(out, vec!["value", "a", "b", "x", "ok", "trim_both"]);
    }

    #[test]
    fn vars_declared_through_filters() {
        assert_eq!(template_vars(None, "{{ x | round }}"), vec!["x"]);
        assert_eq!(template_vars(None, "{{ eta | round | int }}"), vec!["eta"]);
    }

    #[test]
    fn vars_declared_through_arithmetic() {
        assert_eq!(
            template_vars(None, "in {{ (x / 60) | int }} min"),
            vec!["x"]
        );
    }

    #[test]
    fn vars_declared_through_inline_if() {
        assert_eq!(
            template_vars(None, "{{ x if y else z }}"),
            vec!["x", "y", "z"]
        );
    }

    #[test]
    fn vars_leaf_declared_through_filter() {
        assert_eq!(template_vars(None, "{{ vars.x | upper }}"), vec!["x"]);
        assert_eq!(
            template_vars(Some("{{ vars.x | upper }}"), "{{ x }} {{ vars.y ~ '!' }}"),
            vec!["x", "y"]
        );
    }

    #[test]
    fn vars_skip_assigned_names_globals_and_literals() {
        let out = template_vars(
            None,
            "{% set total = a * 2 %}{{ total }} \
             {% for item in items %}{{ item }}{% endfor %} \
             {{ range(3) | list }} {{ 'lit_eral' ~ \"q_uoted\" }} {{ msg.eta }}",
        );
        assert_eq!(out, vec!["a", "items"]);
    }

    #[test]
    fn declared_example_supports_arithmetic() {
        // Preview school: the declared example is a string; it must behave
        // as a number in expressions and still render unchanged when bare.
        let m = render(
            Some("{{ eta }} s"),
            "in {{ (eta / 60) | int }} min, {{ temp | round }}",
            &vars(&[("eta", "120"), ("temp", "21.6")]),
            json!({}),
            json!({}),
        )
        .unwrap();
        assert_eq!(m.subject.as_deref(), Some("120 s"));
        assert_eq!(m.body, "in 2 min, 22.0");
    }

    #[test]
    fn payload_value_supports_filters() {
        // Runtime school: the accumulated payload carries typed values.
        let m = render(
            None,
            "{{ (eta / 60) | int }} {{ vars.name | upper }}",
            &vars(&[("name", "pump")]),
            json!({"eta": 300}),
            json!({}),
        )
        .unwrap();
        assert_eq!(m.body, "5 PUMP");
    }

    #[test]
    fn typed_var_keeps_non_canonical_strings() {
        assert_eq!(typed_var("80"), json!(80));
        assert_eq!(typed_var("82.5"), json!(82.5));
        assert_eq!(typed_var("true"), json!("true"));
        assert_eq!(typed_var("007"), json!("007"));
        assert_eq!(typed_var("1e3"), json!("1e3"));
        assert_eq!(typed_var("NaN"), json!("NaN"));
        assert_eq!(typed_var("pump"), json!("pump"));
    }

    #[test]
    fn vars_expression_non_fermee_ignoree() {
        assert_eq!(
            template_vars(None, "incomplet {{ value"),
            Vec::<String>::new()
        );
    }
}
