//! Sandboxed Starlark executor — **shared lib** used by the `pnex-starlark` node
//! and by the `pnex-flow-runtime --test-function` CLI (test ≡ runtime parity
//! guaranteed by construction).

use std::time::{Duration, Instant};

use starlark::environment::{Globals, LibraryExtension, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};

/// Result of a Starlark function execution.
#[derive(Debug)]
pub struct ExecOutcome {
    /// Raw return value of `handle` (JSON) — `None` = `handle`
    /// returned `None` (message dropped, no port).
    pub return_json: Option<serde_json::Value>,
    /// Declared outputs (`@output`) — drive the key→port routing of the
    /// object return (same contract as the JS wrapper).
    pub outputs: Vec<pnex_core::FunctionOutput>,
    pub logs: Vec<String>,
    pub duration_ms: u64,
}

/// Structured compile-only validation (editor error bar):
/// directives → syntax (position extracted from the Display `file:line:col`)
/// → `compile_check` (missing handle / module evaluation error).
/// No execution of user code beyond the evaluation of the
/// defs-only module (already the contract of `compile_check`).
pub fn check(code: &str) -> Vec<pnex_core::FunctionDiagnostic> {
    use pnex_core::FunctionDiagnostic;
    let mut diagnostics = Vec::new();
    if let Err(e) = pnex_core::parse_directives(code) {
        diagnostics.push(FunctionDiagnostic {
            line: Some(e.line),
            col: None,
            message: format!("directives: {e}"),
        });
        return diagnostics;
    }
    match AstModule::parse("pnex_function.star", code.to_owned(), &Dialect::Standard) {
        Err(e) => {
            let text = format!("syntax: {e}");
            let (line, col) = extract_pos(&text);
            diagnostics.push(FunctionDiagnostic {
                line,
                col,
                message: text,
            });
        }
        Ok(_) => {
            if let Err(msg) = compile_check(code) {
                // compile_check re-parse (microseconds); remaining errors are
                // module-eval or missing-`handle` — no source position.
                diagnostics.push(FunctionDiagnostic {
                    line: None,
                    col: None,
                    message: msg,
                });
            }
        }
    }
    diagnostics
}

/// Stand-in of the driver's `pack_setting` for the checks.
const SETTING_STUB: &str = "def pack_setting(key, default = None):\n    return default\n";

/// Evaluates the module (defs only — Starlark without I/O) and requires `handle`.
/// Called at **node build**: an error here = tab rejected at the deploy's
/// `--check` pre-flight (400 `engine_load`).
pub fn compile_check(code: &str) -> Result<(), String> {
    let sig = pnex_core::parse_directives(code).map_err(|e| format!("directives: {e}"))?;
    let _ = sig;
    // `pack_setting` exists at run time (driver): declared here too, after the
    // code so positions are unchanged.
    let script = format!("{}\n{SETTING_STUB}", code.trim_end());
    let ast = AstModule::parse("pnex_function.star", script, &Dialect::Standard)
        .map_err(|e| format!("syntax: {e}"))?;
    let globals = Globals::extended_by(&[LibraryExtension::Json]);
    Module::with_temp_heap(|module| {
        let mut eval = Evaluator::new(&module);
        apply_limits(&mut eval, Duration::from_millis(200));
        if let Err(e) = eval.eval_module(ast, &globals) {
            return Err(format!("module evaluation: {e}"));
        }
        match module.get("handle") {
            Some(v) if v.get_type() == "function" => Ok(()),
            Some(other) => Err(format!(
                "`handle` must be a function, found \"{}\"",
                other.get_type()
            )),
            None => Err("the code must define `def handle(inputs, msg):` — not found".to_string()),
        }
    })
}

/// Runs `handle(inputs, msg)` sandboxed: directive parsing, `inputs`
/// object built from the declared inputs (each reads
/// `payload.<name>`), Starlark driver that JSON-encodes the return (text
/// bridge — no Value ↔ JSON conversion in Rust), Evaluator limits with a
/// wall-time deadline.
pub fn execute(
    code: &str,
    msg: &serde_json::Value,
    timeout_ms: u64,
) -> anyhow::Result<ExecOutcome> {
    execute_with(code, msg, timeout_ms, &serde_json::Map::new())
}

/// [`execute`] with the org's pack settings (D368), read by the code with
/// `pack_setting("pack.key", default)` inside its functions.
pub fn execute_with(
    code: &str,
    msg: &serde_json::Value,
    timeout_ms: u64,
    settings: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<ExecOutcome> {
    let started = Instant::now();
    let sig = pnex_core::parse_directives(code)?;
    let inputs_obj = pnex_core::build_inputs(&sig.inputs, msg);
    let script = build_script(code, &inputs_obj, msg, settings);

    let timeout = Duration::from_millis(timeout_ms);
    let ast = AstModule::parse("pnex_function.star", script, &Dialect::Standard)
        .map_err(|e| anyhow::anyhow!("syntax: {e}"))?;
    let globals = Globals::extended_by(&[LibraryExtension::Json]);
    Module::with_temp_heap(|module| {
        let mut eval = Evaluator::new(&module);
        apply_limits(&mut eval, timeout);
        eval.eval_module(ast, &globals)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let out = module.get("pnex_out");
        let return_json = match out {
            None => None,
            Some(v) if v.get_type() == "NoneType" => None,
            Some(v) => {
                let text = v
                    .unpack_str()
                    .ok_or_else(|| anyhow::anyhow!("pnex_out: JSON string expected"))?;
                Some(serde_json::from_str(text)?)
            }
        };
        Ok(ExecOutcome {
            return_json,
            outputs: sig.outputs,
            logs: Vec::new(),
            duration_ms: started.elapsed().as_millis() as u64,
        })
    })
}

/// Builds the full script: user code (defining `handle`) +
/// generated driver (json.decode of the arguments, call, json.encode of the return).
fn build_script(
    code: &str,
    inputs_obj: &serde_json::Map<String, serde_json::Value>,
    msg: &serde_json::Value,
    settings: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let inputs_lit =
        starlark_str_lit(&serde_json::to_string(&inputs_obj).unwrap_or_else(|_| "{}".into()));
    let msg_lit = starlark_str_lit(&serde_json::to_string(msg).unwrap_or_else(|_| "{}".into()));
    let settings_lit =
        starlark_str_lit(&serde_json::to_string(settings).unwrap_or_else(|_| "{}".into()));
    // After the user code so its line numbers stay those of the editor:
    // `pack_setting` is called from inside functions, resolved at call time.
    format!(
        "{}\n# ── PNEX driver (generated) ──\n\
         pnex_settings = json.decode({settings_lit})\n\
         def pack_setting(key, default = None):\n    return pnex_settings.get(key, default)\n\
         pnex_inputs = json.decode({inputs_lit})\n\
         pnex_msg = json.decode({msg_lit})\n\
         pnex_r = handle(pnex_inputs, pnex_msg)\n\
         pnex_out = None if pnex_r == None else json.encode(pnex_r)\n",
        code.trim_end()
    )
}

/// Single-quoted Starlark string literal — backslashes and quotes
/// escaped (serde's JSON has no raw newline, already \\n).
fn starlark_str_lit(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Evaluator limits: ticks (calls + loop back-edges), heap, callstack
/// and wall-time deadline via `check_cancelled`.
fn apply_limits(eval: &mut Evaluator, timeout: Duration) {
    let _ = eval.set_max_tick_count(10_000_000);
    let _ = eval.set_max_heap_size(64 * 1024 * 1024);
    let _ = eval.set_max_callstack_size(100);
    let deadline = Instant::now() + timeout;
    eval.set_check_cancelled(Box::new(move || Instant::now() >= deadline));
}

/// Maps the raw return of `handle` to outgoing msgs per port — the same
/// contract as the JS wrapper: null → no msg; **object + declared
/// outputs → key→port routing** (missing/null key = silent port); object
/// without declared outputs → payload replaced (merged if the object carries
/// `payload`); array → one element per port; bare value → port 0.
pub fn map_return(
    return_json: &Option<serde_json::Value>,
    outputs: &[pnex_core::FunctionOutput],
    incoming: &serde_json::Value,
) -> Vec<Option<serde_json::Value>> {
    match return_json {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|x| match x {
                serde_json::Value::Null => None,
                other => Some(with_payload(incoming, other)),
            })
            .collect(),
        Some(serde_json::Value::Object(map)) if !outputs.is_empty() => outputs
            .iter()
            .map(|o| match map.get(&o.name) {
                None | Some(serde_json::Value::Null) => None,
                Some(v) => Some(set_payload(incoming, v)),
            })
            .collect(),
        Some(other) => vec![Some(with_payload(incoming, other))],
    }
}

/// Outgoing msg = incoming msg with `payload` set as is (key→port
/// routing: the value is the payload, no special merge).
fn set_payload(incoming: &serde_json::Value, v: &serde_json::Value) -> serde_json::Value {
    let mut obj = incoming.as_object().cloned().unwrap_or_default();
    obj.insert("payload".to_string(), v.clone());
    serde_json::Value::Object(obj)
}

/// Outgoing msg = incoming msg with `payload` replaced; if the value is an
/// object carrying `payload`, it is merged into the msg (full control).
fn with_payload(incoming: &serde_json::Value, r: &serde_json::Value) -> serde_json::Value {
    let mut obj = incoming.as_object().cloned().unwrap_or_default();
    match r {
        serde_json::Value::Object(map) if map.contains_key("payload") => {
            for (k, v) in map {
                obj.insert(k.clone(), v.clone());
            }
        }
        other => {
            obj.insert("payload".to_string(), other.clone());
        }
    }
    serde_json::Value::Object(obj)
}

/// Best-effort extraction of the first `file:line` or
/// `file:line:col` position from a starlark error Display, without a
/// regex dependency. 1-based line/column; (None, None) if absent.
fn extract_pos(text: &str) -> (Option<u32>, Option<u32>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let line: u32 = text[i + 1..j].parse().unwrap_or(0);
            if line > 0 {
                let mut col = None;
                if j < bytes.len()
                    && bytes[j] == b':'
                    && j + 1 < bytes.len()
                    && bytes[j + 1].is_ascii_digit()
                {
                    let c0 = j + 1;
                    let mut c1 = c0;
                    while c1 < bytes.len() && bytes[c1].is_ascii_digit() {
                        c1 += 1;
                    }
                    let c: u32 = text[c0..c1].parse().unwrap_or(0);
                    if c > 0 {
                        col = Some(c);
                    }
                }
                return (Some(line), col);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec(code: &str, msg: serde_json::Value, timeout_ms: u64) -> anyhow::Result<ExecOutcome> {
        execute(code, &msg, timeout_ms)
    }

    #[test]
    fn compile_check_ok_handle_defini() {
        compile_check("def handle(inputs, msg):\n    return inputs").unwrap();
    }

    #[test]
    fn compile_check_handle_absent() {
        let e = compile_check("x = 1\n").unwrap_err();
        assert!(e.contains("not found"), "{e}");
    }

    #[test]
    fn compile_check_syntaxe_invalide() {
        assert!(compile_check("def handle(:\n    pass").is_err());
    }

    #[test]
    fn compile_check_directives_invalides() {
        let e =
            compile_check("def handle(inputs, msg):\n    return 0\n// @input a float").unwrap_err();
        assert!(e.contains("directives"), "{e}");
    }

    #[test]
    fn check_ok_sans_diagnostics() {
        assert!(check("def handle(inputs, msg):\n    return inputs").is_empty());
    }

    #[test]
    fn check_syntaxe_avec_position() {
        let diags = check("def handle(:\n    pass");
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.starts_with("syntax: "),
            "{}",
            diags[0].message
        );
        assert_eq!(diags[0].line, Some(1), "{:?}", diags[0]);
    }

    #[test]
    fn check_directive_avec_ligne() {
        let diags = check("# @input a float\ndef handle(inputs, msg):\n    return 0\n");
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.starts_with("directives: "),
            "{}",
            diags[0].message
        );
        assert_eq!(diags[0].line, Some(1));
    }

    #[test]
    fn check_handle_absent_sans_position() {
        let diags = check("x = 1\n");
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("not found"),
            "{}",
            diags[0].message
        );
        assert_eq!(diags[0].line, None);
    }

    #[test]
    fn extract_pos_premier_match() {
        assert_eq!(
            extract_pos("pnex_function.star:2:11: message"),
            (Some(2), Some(11))
        );
        assert_eq!(extract_pos("aucune position"), (None, None));
        assert_eq!(extract_pos(":0:0"), (None, None));
        assert_eq!(extract_pos("fin:12"), (Some(12), None));
    }

    #[test]
    fn execute_valeurs_et_inputs_binds() {
        // Each declared input implicitly reads payload.<name>; missing →
        // declared default (0); otherwise null.
        let out = execute(
            "# @input t number=0\ndef handle(inputs, msg):\n    return {\"sum\": inputs[\"t\"] + 1, \"topic\": msg[\"topic\"]}",
            &serde_json::json!({"payload": {"t": 41}, "topic": "s1"}),
            1000,
        )
        .unwrap();
        let ret = out.return_json.expect("return expected");
        assert_eq!(ret["sum"], 42);
        assert_eq!(ret["topic"], "s1");
    }

    #[test]
    fn execute_topic_route_le_payload_vers_l_entree() {
        // msg.topic naming the declared input → routed incoming payload
        // (parity with the JS wrapper — tagger inserted by the projection).
        let out = execute(
            "# @input t number=0\ndef handle(inputs, msg):\n    return {\"v\": inputs[\"t\"]}",
            &serde_json::json!({"topic": "t", "payload": 41}),
            1000,
        )
        .unwrap();
        let ret = out.return_json.expect("return expected");
        assert_eq!(ret["v"], 41);
    }

    #[test]
    fn setting_reads_the_org_value_or_the_default() {
        let code = "def handle(inputs, msg):\n    return {\"c\": pack_setting(\"p.confirm\", 80), \"o\": pack_setting(\"p.other\", 1)}";
        compile_check(code).unwrap();
        let mut s = serde_json::Map::new();
        s.insert("p.confirm".into(), serde_json::json!(70));
        let r = execute_with(code, &serde_json::json!({}), 1000, &s).unwrap();
        assert_eq!(r.return_json, Some(serde_json::json!({"c": 70, "o": 1})));
        let r = execute(code, &serde_json::json!({}), 1000).unwrap();
        assert_eq!(r.return_json, Some(serde_json::json!({"c": 80, "o": 1})));
    }

    #[test]
    fn execute_null_et_erreur_runtime() {
        // None → message dropped.
        let out = exec(
            "def handle(inputs, msg):\n    return None",
            serde_json::json!({}),
            500,
        )
        .unwrap();
        assert!(out.return_json.is_none());
        // Nonexistent variable → clean error, never a panic.
        let err = exec(
            "def handle(inputs, msg):\n    return variable_inexistante",
            serde_json::json!({}),
            500,
        )
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn boucle_infinie_tuee_par_deadline() {
        let err = exec(
            "def handle(inputs, msg):\n    x = 0\n    for i in range(100000000):\n        x = x + 1\n    return x",
            serde_json::json!({}),
            100,
        )
        .unwrap_err();
        assert!(!err.to_string().is_empty(), "deadline triggered");
    }

    #[test]
    fn map_return_contrat_complet() {
        let incoming = serde_json::json!({"payload": 1, "topic": "t"});
        // null → no msg.
        assert!(map_return(&None, &[], &incoming).is_empty());
        assert!(map_return(&Some(serde_json::Value::Null), &[], &incoming).is_empty());
        // object without declared outputs → payload replaced, topic kept.
        let outs = map_return(&Some(serde_json::json!({"v": 2})), &[], &incoming);
        assert_eq!(outs.len(), 1);
        assert_eq!(outs[0].as_ref().unwrap()["topic"], "t");
        assert_eq!(outs[0].as_ref().unwrap()["payload"]["v"], 2);
        // array → one element per port, null = silent port.
        let outs = map_return(
            &Some(serde_json::json!([{"v": 1}, null, "x"])),
            &[],
            &incoming,
        );
        assert_eq!(outs.len(), 3);
        assert!(outs[1].is_none());
        assert_eq!(outs[2].as_ref().unwrap()["payload"], "x");
    }

    #[test]
    fn map_return_route_par_cle_sur_sorties_declarees() {
        let out = |name: &str| pnex_core::FunctionOutput {
            name: name.into(),
            ty: pnex_core::FunctionType::Bool,
            desc: None,
        };
        let outputs = vec![out("out_bool"), out("out_temp")];
        let incoming = serde_json::json!({"payload": 1, "topic": "t"});
        // Object + declared outputs: each key feeds its port
        // (payload = the value), silent port if the key is missing/null.
        let outs = map_return(
            &Some(serde_json::json!({"out_bool": true, "out_temp": 21.5})),
            &outputs,
            &incoming,
        );
        assert_eq!(outs.len(), 2);
        assert_eq!(outs[0].as_ref().unwrap()["payload"], true);
        assert_eq!(outs[1].as_ref().unwrap()["payload"], 21.5);
        // Missing key → silent port; null key → silent port; unknown key
        // ignored; bare value (outside an object) → port 0.
        let outs = map_return(
            &Some(serde_json::json!({"out_temp": 3, "inconnu": 9})),
            &outputs,
            &incoming,
        );
        assert_eq!(outs.len(), 2);
        assert!(outs[0].is_none(), "out_bool missing → silent");
        assert_eq!(outs[1].as_ref().unwrap()["payload"], 3);
        let outs = map_return(&Some(serde_json::json!(true)), &outputs, &incoming);
        assert_eq!(outs.len(), 1);
        assert_eq!(outs[0].as_ref().unwrap()["payload"], true);
    }
}
