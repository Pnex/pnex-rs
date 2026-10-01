//! Test en live des fonctions du registre « Fonctions » — sous-commande
//! `pnex-flow-runtime --test-function <request.json>`.
//!
//! Le backend écrit la requête dans un fichier temp, spawn ce binaire et lit
//! **une ligne JSON** en sortie. Parité test ≡ runtime :
//! - Starlark : même exécuteur que le nœud `pnex-starlark` (même crate) ;
//! - JS : miroir rquickjs du nœud `function` vendor — même wrap
//!   `__el_user_func`, le wrapper évalué est le texte généré par
//!   `build_js_wrapper` (pnex-core).

use std::time::Instant;

use pnex_core::{FunctionExecRequest, FunctionLanguage, FunctionTestResponse};

/// Point d'entrée du mode `--test-function` : lit la requête, exécute la
/// fonction (Starlark = exécuteur nœud ; JS = miroir rquickjs) et imprime
/// **une ligne JSON** `FunctionTestResponse`. Exit 0 même si `ok = false`
/// (le résultat EST la sortie) ; exit 1 si la requête est illisible.
pub async fn run(request_path: &str) -> std::process::ExitCode {
    let raw = match std::fs::read_to_string(request_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("test-function : requête illisible : {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let req: FunctionExecRequest = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("test-function : requête invalide : {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let started = Instant::now();
    let response = match req.language {
        FunctionLanguage::Starlark => {
            let code = req.code.clone().unwrap_or_default();
            let msg = req.msg.clone();
            let timeout_ms = req.timeout_ms;
            let res = tokio::task::spawn_blocking(move || {
                pnex_node_starlark::executor::execute(&code, &msg, timeout_ms)
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("tâche d'exécution : {e}")));
            starlark_response(res, &req.msg, started.elapsed().as_millis() as u64)
        }
        FunctionLanguage::Js => {
            let wrapper = req.js_wrapper.clone().unwrap_or_default();
            let res = exec_js(&wrapper, &req.msg, req.timeout_ms).await;
            match res {
                Ok((outputs, logs)) => FunctionTestResponse {
                    ok: true,
                    outputs,
                    logs,
                    duration_ms: started.elapsed().as_millis() as u64,
                    error: None,
                },
                Err(e) => script_error(e, started.elapsed().as_millis() as u64),
            }
        }
    };

    println!("{}", serde_json::to_string(&response).unwrap_or_default());
    std::process::ExitCode::SUCCESS
}

/// Starlark : `ok:false` pour une erreur script (exécuteur retourne Err),
/// `ok:true` + mapping ports sinon.
fn starlark_response(
    res: anyhow::Result<pnex_node_starlark::executor::ExecOutcome>,
    msg: &serde_json::Value,
    duration_ms: u64,
) -> FunctionTestResponse {
    match res {
        Ok(outcome) => {
            let outs = pnex_node_starlark::executor::map_return(
                &outcome.return_json,
                &outcome.outputs,
                msg,
            );
            FunctionTestResponse {
                ok: true,
                // Slots conservés : null = port muet (l'alignement
                // port↔@output déclaré est l'info affichée au panneau).
                outputs: outs
                    .into_iter()
                    .map(|o| o.unwrap_or(serde_json::Value::Null))
                    .collect(),
                logs: outcome.logs,
                duration_ms,
                error: None,
            }
        }
        Err(e) => script_error(e, duration_ms),
    }
}

fn script_error(e: anyhow::Error, duration_ms: u64) -> FunctionTestResponse {
    FunctionTestResponse {
        ok: false,
        outputs: Vec::new(),
        logs: Vec::new(),
        duration_ms,
        error: Some(e.to_string()),
    }
}

/// Miroir rquickjs du nœud `function` vendor : runtime frais, wrap
/// `async function __el_user_func(msg) { … }` (école vendor
/// function/mod.rs:164-172), console shim capturée, options
/// `{promise: true, strict: false}` (make_eval_options vendor). Le pont
/// Rust ↔ JS passe par JSON.stringify (aucune conversion valeur par valeur).
async fn exec_js(
    wrapper: &str,
    msg: &serde_json::Value,
    timeout_ms: u64,
) -> anyhow::Result<(Vec<serde_json::Value>, Vec<String>)> {
    use rquickjs::{AsyncContext, AsyncRuntime};

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let rt = AsyncRuntime::new()?;
    // Mémoire/stack best-effort : la limite fiable reste l'interrupt handler
    // (les features allocator du graphe peuvent rendre memory_limit no-op).
    // Méthodes async sur AsyncRuntime — doivent être awaited.
    rt.set_memory_limit(128 * 1024 * 1024).await;
    rt.set_max_stack_size(1024 * 1024).await;
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)))
        .await;

    let ctx = AsyncContext::full(&rt).await?;
    let script = build_runner(wrapper);
    let msg_text = serde_json::to_string(msg)?;

    let result = rquickjs::async_with!(ctx => |ctx| {
        use rquickjs::context::EvalOptions;
        use rquickjs::Function;
        let mut opts = EvalOptions::default();
        opts.promise = true;
        opts.strict = false;
        ctx.eval_with_options::<(), _>(script.as_bytes(), opts)?;
        let run: Function = ctx.globals().get("__pnex_run")?;
        let promised: rquickjs::Promise = run.call((msg_text,))?;
        let res: Option<String> = promised.into_future().await?;
        Ok::<Option<String>, rquickjs::Error>(res)
    })
    .await
    .map_err(|e| anyhow::anyhow!("exécution JS : {e}"))?;

    let parsed: serde_json::Value = match result {
        None => serde_json::json!({"outputs": null, "logs": []}),
        Some(text) => serde_json::from_str(&text)?,
    };
    // Normalisation par port : null = port muet ; msg complet → son
    // `payload` (le panneau affiche la valeur, pas le msg d'enveloppe).
    let outputs = match parsed.get("outputs") {
        Some(serde_json::Value::Null) | None => Vec::new(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                serde_json::Value::Null => serde_json::Value::Null,
                serde_json::Value::Object(map) => map
                    .get("payload")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                other => other.clone(),
            })
            .collect(),
        Some(other) => vec![other.clone()],
    };
    let logs = parsed
        .get("logs")
        .and_then(|l| serde_json::from_value::<Vec<String>>(l.clone()).ok())
        .unwrap_or_default();
    Ok((outputs, logs))
}

/// Construit le script du miroir : console shim + wrap vendor + runner
/// (concat, pas de format! — accolades JS).
fn build_runner(js_wrapper: &str) -> String {
    let mut s = String::with_capacity(js_wrapper.len() + 1024);
    s.push_str("globalThis.pnex_logs = [];\n");
    s.push_str(
        "function __pnex_log(level) {\n\
         \x20 return function () {\n\
         \x20   globalThis.pnex_logs.push(level + \" \" + Array.prototype.map.call(arguments, String).join(\" \"));\n\
         \x20 };\n\
         }\n",
    );
    s.push_str(
        "var console = {\n\
         \x20 log: __pnex_log(\"log\"),\n\
         \x20 info: __pnex_log(\"log\"),\n\
         \x20 warn: __pnex_log(\"warn\"),\n\
         \x20 error: __pnex_log(\"error\"),\n\
         };\n",
    );
    s.push_str("async function __el_user_func(msg) {\n");
    s.push_str(js_wrapper);
    s.push_str("\n}\n");
    s.push_str(
        "async function __pnex_run(msg_json) {\n\
         \x20 const msg = JSON.parse(msg_json);\n\
         \x20 const r = await __el_user_func(msg);\n\
         \x20 if (r === null || r === undefined) return JSON.stringify({\"outputs\": null, \"logs\": globalThis.pnex_logs});\n\
         \x20 return JSON.stringify({\"outputs\": r, \"logs\": globalThis.pnex_logs});\n\
         }\n",
    );
    s
}
