//! `pnex`: admin CLI of a PNeX instance (D251). Every operation of the
//! OpenAPI spec embedded at build time is a `pnex <tag> <action>` command,
//! including the ones the MCP never exposes (deletes, device commands,
//! deploy/stop): those refuse to run without `--yes` (D253). Rights are
//! the caller's own, checked by the server like for the UI.

mod session;

use std::io::{Read, Write};

use anyhow::{bail, Context, Result};
use clap::{Arg, ArgAction, ArgMatches, Command};
use pnex_api_contract::exposure::EXCLUDED;
use reqwest::blocking::Client;
use reqwest::Method;
use serde_json::Value;
use session::Session;

/// The spec of this commit: `tests/openapi.rs` keeps it equal to the server.
const SPEC: &str = include_str!("../../../docs/contracts/openapi.json");
const ORG_HEADER: &str = "X-Org-Id";
/// `EXCLUDED` reasons (text before `:`) that need `--yes` on the CLI.
const DANGEROUS: &[&str] = &[
    "destructive",
    "physical",
    "runtime",
    "firmware",
    "credential",
    "access",
    "outward",
    "sign-off",
    "platform",
    "bulk",
];

struct Param {
    name: String,
    flag: String,
    loc: String,
    required: bool,
    help: String,
}

struct Op {
    tag: String,
    action: String,
    method: Method,
    path: String,
    params: Vec<Param>,
    /// `(content type, required)`.
    body: Option<(String, bool)>,
    about: String,
    long_about: String,
    danger: Option<&'static str>,
}

fn danger(op_id: &str, method: &Method) -> Option<&'static str> {
    let reason = EXCLUDED
        .iter()
        .find(|(_, ops)| ops.contains(&op_id))
        .map(|(reason, _)| *reason)
        .filter(|r| DANGEROUS.contains(&r.split(':').next().unwrap_or("")));
    reason.or((method == Method::DELETE).then_some("destructive: deletes data"))
}

fn kebab(s: &str) -> String {
    s.to_lowercase().replace('_', "-")
}

fn ops() -> Vec<Op> {
    let spec: Value = serde_json::from_str(SPEC).expect("embedded spec parses");
    let mut ops = Vec::new();
    for (path, item) in spec["paths"].as_object().expect("paths") {
        for (method, op) in item.as_object().expect("path item") {
            let id = op["operationId"].as_str().expect("operationId");
            let tag = op["tags"][0].as_str().unwrap_or("misc");
            let action = id
                .strip_prefix(&format!("{}_", tag.replace('-', "_")))
                .unwrap_or(id);
            let method = Method::from_bytes(method.to_uppercase().as_bytes()).expect("method");
            let params = op["parameters"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| {
                    let name = p["name"].as_str().expect("param name").to_string();
                    Param {
                        help: p["description"].as_str().unwrap_or("").to_string(),
                        flag: kebab(&name),
                        loc: p["in"].as_str().unwrap_or("query").to_string(),
                        required: p["required"].as_bool().unwrap_or(false),
                        name,
                    }
                })
                .collect();
            let body = op["requestBody"]["content"].as_object().and_then(|c| {
                let ct = c.keys().next()?.clone();
                Some((ct, op["requestBody"]["required"].as_bool().unwrap_or(false)))
            });
            let danger = danger(id, &method);
            let mut long_about = op["description"].as_str().unwrap_or("").to_string();
            if let Some(reason) = danger {
                long_about.push_str(&format!("\n\nNeeds --yes ({reason})."));
            }
            long_about.push_str(&format!("\n\n{method} {path}  [{id}]"));
            let long_about = long_about.trim_start().to_string();
            ops.push(Op {
                tag: tag.to_string(),
                action: kebab(action),
                about: op["summary"].as_str().unwrap_or(id).to_string(),
                long_about,
                method,
                path: path.clone(),
                params,
                body,
                danger,
            });
        }
    }
    ops
}

fn body_arg(required: bool) -> Arg {
    Arg::new("body")
        .long("body")
        .required(required)
        .value_name("JSON|@FILE|-")
        .help("Request body: inline, @file or - for stdin")
}

fn yes_arg() -> Arg {
    Arg::new("yes")
        .long("yes")
        .action(ArgAction::SetTrue)
        .help("Confirm a destructive, physical or access-changing operation")
}

fn cli(ops: &[Op]) -> Command {
    let mut root = Command::new("pnex")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Admin CLI of a PNeX instance: every API operation, with your own rights")
        .subcommand_required(true)
        .arg(
            Arg::new("org")
                .long("org")
                .global(true)
                .value_parser(clap::value_parser!(i64))
                .help("Organisation id for this call (default: `pnex org <id>`)"),
        )
        .subcommand(
            Command::new("login")
                .about("Sign in to an instance in the browser, once")
                .arg(
                    Arg::new("url")
                        .required(true)
                        .help("Instance URL, e.g. https://pnex.example.org"),
                ),
        )
        .subcommand(Command::new("logout").about("Forget the stored session"))
        .subcommand(
            Command::new("org")
                .about("List your organisations, or set the default one")
                .arg(Arg::new("id").value_parser(clap::value_parser!(i64))),
        )
        .subcommand(
            Command::new("raw")
                .about("Call any route as is (non-GET needs --yes)")
                .arg(Arg::new("method").required(true))
                .arg(
                    Arg::new("path")
                        .required(true)
                        .help("e.g. /api/v1/devices?page=1"),
                )
                .arg(body_arg(false))
                .arg(yes_arg()),
        )
        .subcommand(
            Command::new("import")
                .about("Run an import")
                .subcommand_required(true)
                .subcommand(
                    Command::new("run")
                        .about("Start a run; --wait follows it and exits non-zero unless applied")
                        .arg(Arg::new("id").required(true).help("Import id"))
                        .arg(
                            Arg::new("wait")
                                .long("wait")
                                .action(ArgAction::SetTrue)
                                .help("Wait for the end of the run"),
                        )
                        .arg(yes_arg()),
                ),
        )
        .subcommand(
            Command::new("dataset")
                .about("Load a file into a table")
                .subcommand_required(true)
                .subcommand(
                    Command::new("load")
                        .about("Send a CSV in parts, whatever a proxy allows per request, and wait for its conversion; exits non-zero unless it becomes a version")
                        .arg(Arg::new("id").required(true).help("Dataset id"))
                        .arg(Arg::new("file").required(true).help("Path of the CSV file"))
                        .arg(
                            Arg::new("delimiter")
                                .long("delimiter")
                                .help("One character. Default: detected from the first line"),
                        )
                        .arg(
                            Arg::new("append")
                                .long("append")
                                .action(ArgAction::SetTrue)
                                .help("Add to the table instead of replacing its content"),
                        )
                        .arg(yes_arg()),
                ),
        );
    let mut tags: Vec<&str> = ops.iter().map(|o| o.tag.as_str()).collect();
    tags.dedup();
    tags.sort_unstable();
    tags.dedup();
    for tag in tags {
        let n = ops.iter().filter(|o| o.tag == tag).count();
        let mut group = Command::new(tag.to_string())
            .about(format!("{n} operation(s)"))
            .subcommand_required(true);
        for op in ops.iter().filter(|o| o.tag == tag) {
            let mut cmd = Command::new(op.action.clone())
                .about(op.about.clone())
                .long_about(op.long_about.clone());
            for p in op.params.iter().filter(|p| p.name != ORG_HEADER) {
                cmd = cmd.arg(
                    Arg::new(p.flag.clone())
                        .long(p.flag.clone())
                        .required(p.required)
                        .action(ArgAction::Append)
                        .help(format!("[{}] {}", p.loc, p.help)),
                );
            }
            if let Some((ct, required)) = &op.body {
                cmd = cmd.arg(
                    body_arg(*required).help(format!("Request body ({ct}): inline, @file or -")),
                );
            }
            if op.danger.is_some() {
                cmd = cmd.arg(yes_arg());
            }
            group = group.subcommand(cmd);
        }
        root = root.subcommand(group);
    }
    root
}

fn read_body(arg: &str) -> Result<Vec<u8>> {
    if arg == "-" {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf)?;
        Ok(buf)
    } else if let Some(file) = arg.strip_prefix('@') {
        std::fs::read(file).with_context(|| format!("cannot read {file}"))
    } else {
        Ok(arg.as_bytes().to_vec())
    }
}

/// Path segment encoding (RFC 3986 unreserved kept).
fn encode(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn send(
    http: &Client,
    session: &mut Session,
    org: Option<i64>,
    method: Method,
    url: reqwest::Url,
    headers: Vec<(String, String)>,
    body: Option<(String, Vec<u8>)>,
) -> Result<()> {
    let token = session.token(http)?.to_string();
    let mut req = http.request(method, url).bearer_auth(token);
    if let Some(org) = org {
        req = req.header(ORG_HEADER, org.to_string());
    }
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some((ct, bytes)) = body {
        if ct.contains("json") {
            serde_json::from_slice::<Value>(&bytes).context("--body is not valid JSON")?;
        }
        req = req.header(reqwest::header::CONTENT_TYPE, ct).body(bytes);
    }
    let res = req.send()?;
    let status = res.status();
    let is_json = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("json"));
    let bytes = res.bytes()?;
    let mut out: Box<dyn Write> = if status.is_success() {
        Box::new(std::io::stdout())
    } else {
        Box::new(std::io::stderr())
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(v) if is_json => writeln!(out, "{}", serde_json::to_string_pretty(&v)?)?,
        _ => out.write_all(&bytes)?,
    }
    if !status.is_success() {
        bail!("HTTP {status}");
    }
    Ok(())
}

fn run_op(http: &Client, op: &Op, m: &ArgMatches, org: Option<i64>) -> Result<()> {
    if let Some(reason) = op.danger {
        if !m.get_flag("yes") {
            bail!("refused without --yes: {reason}");
        }
    }
    let mut session = Session::load()?;
    let mut path = op.path.clone();
    let mut query = Vec::new();
    let mut headers = Vec::new();
    for p in op.params.iter().filter(|p| p.name != ORG_HEADER) {
        for v in m.get_many::<String>(&p.flag).into_iter().flatten() {
            match p.loc.as_str() {
                "path" => path = path.replace(&format!("{{{}}}", p.name), &encode(v)),
                "header" => headers.push((p.name.clone(), v.clone())),
                _ => query.push((p.name.clone(), v.clone())),
            }
        }
    }
    let org_param = op.params.iter().find(|p| p.name == ORG_HEADER);
    let org = org_param.and(org.or(session.org));
    if org_param.is_some_and(|p| p.required) && org.is_none() {
        bail!("no organisation: pick one with `pnex org <id>` or pass --org");
    }
    let mut url = reqwest::Url::parse(&format!("{}{path}", session.url))?;
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    // `--body` only exists on operations that take one.
    let body = match &op.body {
        Some((ct, _)) => match m.get_one::<String>("body") {
            Some(arg) => Some((ct.clone(), read_body(arg)?)),
            None => None,
        },
        None => None,
    };
    send(
        http,
        &mut session,
        org,
        op.method.clone(),
        url,
        headers,
        body,
    )
}

fn run_org(http: &Client, id: Option<i64>) -> Result<()> {
    let mut session = Session::load()?;
    let token = session.token(http)?.to_string();
    let info: Value = http
        .get(format!("{}/api/v1/user-info", session.url))
        .bearer_auth(token)
        .send()?
        .error_for_status()?
        .json()?;
    let orgs = info["orgs"].as_array().cloned().unwrap_or_default();
    if let Some(id) = id {
        if !orgs.iter().any(|o| o["id"].as_i64() == Some(id)) {
            bail!("you are not a member of organisation {id}");
        }
        session.org = Some(id);
        session.save()?;
    }
    for o in &orgs {
        let mark = if o["id"].as_i64() == session.org {
            "*"
        } else {
            " "
        };
        println!(
            "{mark} {:>6}  {:<10} {}",
            o["id"],
            o["role"].as_str().unwrap_or(""),
            o["name"].as_str().unwrap_or("")
        );
    }
    if session.org.is_none() {
        eprintln!("No default organisation: `pnex org <id>`.");
    }
    Ok(())
}

/// How a run ended, once it has: `Err` says what stopped it.
fn run_end(run: &Value) -> Option<Result<(), String>> {
    let state = run["state"].as_str().unwrap_or("unknown");
    match (state, run["state_code"].as_str()) {
        ("running", _) => None,
        ("applied", _) => Some(Ok(())),
        (_, Some(code)) => Some(Err(format!("{state}: {code}"))),
        _ => Some(Err(state.to_string())),
    }
}

fn call_json(
    http: &Client,
    session: &mut Session,
    org: Option<i64>,
    method: Method,
    path: &str,
) -> Result<(reqwest::StatusCode, Value)> {
    let token = session.token(http)?.to_string();
    let mut req = http
        .request(method, format!("{}{path}", session.url))
        .bearer_auth(token);
    if let Some(org) = org {
        req = req.header(ORG_HEADER, org.to_string());
    }
    let res = req.send()?;
    let status = res.status();
    Ok((status, serde_json::from_slice(&res.bytes()?)?))
}

/// Well under what a proxy lets through in one request (Cloudflare: 100 MB).
const UPLOAD_PART_BYTES: u64 = 32 * 1024 * 1024;
const UPLOAD_ATTEMPTS: u32 = 4;
/// A slow link on a whole part: the client's default of 30 s would cut it.
const UPLOAD_PART_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Reads up to one part. Empty at the end of the file.
fn read_part(file: &mut impl std::io::Read) -> Result<Vec<u8>> {
    let mut part = Vec::new();
    file.take(UPLOAD_PART_BYTES).read_to_end(&mut part)?;
    Ok(part)
}

fn send_part(
    http: &Client,
    session: &mut Session,
    org: Option<i64>,
    path: &str,
    part: &[u8],
) -> Result<()> {
    let mut attempt = 1;
    loop {
        let token = session.token(http)?.to_string();
        let mut req = http
            .put(format!("{}{path}", session.url))
            .bearer_auth(token)
            .timeout(UPLOAD_PART_TIMEOUT)
            .body(part.to_vec());
        if let Some(org) = org {
            req = req.header(ORG_HEADER, org.to_string());
        }
        let why = match req.send() {
            Ok(res) if res.status().is_success() => return Ok(()),
            Ok(res) if res.status().is_client_error() => {
                bail!(
                    "HTTP {} on {path}: {}",
                    res.status(),
                    res.text().unwrap_or_default()
                )
            }
            Ok(res) => res.status().to_string(),
            Err(e) => e.to_string(),
        };
        if attempt >= UPLOAD_ATTEMPTS {
            bail!("{path} failed {attempt} times: {why}");
        }
        attempt += 1;
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

/// `pnex dataset load`: the conversion ends on the server whatever happens here.
#[allow(clippy::too_many_arguments)]
fn load_dataset(
    http: &Client,
    session: &mut Session,
    org: Option<i64>,
    id: &str,
    file: &str,
    delimiter: Option<&String>,
    append: bool,
    patience: u32,
) -> Result<()> {
    let mut source = std::fs::File::open(file).with_context(|| format!("cannot read {file}"))?;
    let base = format!("/api/v1/datasets/{}/uploads", encode(id));
    let input = serde_json::json!({
        "kind": if append { "append" } else { "snapshot" },
        "delimiter": delimiter,
    });
    let (status, upload) = post_json(http, session, org, &base, input)?;
    if !status.is_success() {
        bail!("HTTP {status}: {upload}");
    }
    let upload_id = upload["id"]
        .as_str()
        .context("upload without an id")?
        .to_string();
    let path = format!("{base}/{}", encode(&upload_id));
    let mut parts = 0;
    loop {
        let part = read_part(&mut source)?;
        // An empty file too is a part: the server says why it refuses it.
        if part.is_empty() && parts > 0 {
            break;
        }
        parts += 1;
        send_part(http, session, org, &format!("{path}/parts/{parts}"), &part)?;
        eprintln!("part {parts} sent");
        if (part.len() as u64) < UPLOAD_PART_BYTES {
            break;
        }
    }
    let close = serde_json::json!({ "parts": parts });
    let (status, mut upload) = post_json(http, session, org, &format!("{path}/complete"), close)?;
    if !status.is_success() {
        bail!("HTTP {status}: {upload}");
    }
    let mut failures = 0;
    while !matches!(upload["state"].as_str(), Some("done" | "failed")) {
        std::thread::sleep(std::time::Duration::from_secs(2));
        match call_json(http, session, org, Method::GET, &path) {
            Ok((status, body)) if status.is_success() => {
                failures = 0;
                upload = body;
            }
            Ok((status, _)) if status.is_client_error() => bail!("HTTP {status} on {path}"),
            failed => {
                failures += 1;
                if failures >= patience {
                    let why = failed.map_or_else(|e| e.to_string(), |(s, _)| s.to_string());
                    bail!("upload {upload_id} still unknown after {failures} polls: {why}");
                }
            }
        }
    }
    println!("{}", serde_json::to_string_pretty(&upload)?);
    if upload["state"] == "failed" {
        bail!(
            "not loaded: {}",
            upload["code"].as_str().unwrap_or("unknown")
        );
    }
    Ok(())
}

fn post_json(
    http: &Client,
    session: &mut Session,
    org: Option<i64>,
    path: &str,
    body: Value,
) -> Result<(reqwest::StatusCode, Value)> {
    let token = session.token(http)?.to_string();
    let mut req = http
        .post(format!("{}{path}", session.url))
        .bearer_auth(token)
        .json(&body);
    if let Some(org) = org {
        req = req.header(ORG_HEADER, org.to_string());
    }
    let res = req.send()?;
    let status = res.status();
    Ok((status, serde_json::from_slice(&res.bytes()?)?))
}

/// `pnex import run`: the run ends on the server whatever happens here, so a poll that
/// fails (a pod replaced, a busy server) is retried for `patience` polls in a row.
fn run_import(
    http: &Client,
    session: &mut Session,
    org: Option<i64>,
    id: &str,
    wait: bool,
    every: std::time::Duration,
    patience: u32,
) -> Result<()> {
    let base = format!("/api/v1/ontology/imports/{}/runs", encode(id));
    let (status, mut run) = call_json(http, session, org, Method::POST, &base)?;
    if !status.is_success() {
        bail!("HTTP {status}: {run}");
    }
    if wait {
        let run_id = run["id"].as_str().context("run without an id")?.to_string();
        let path = format!("{base}/{}", encode(&run_id));
        let mut failures = 0;
        while run_end(&run).is_none() {
            std::thread::sleep(every);
            match call_json(http, session, org, Method::GET, &path) {
                Ok((status, mut body)) if status.is_success() => {
                    failures = 0;
                    // The start answers the run, the poll a preview that holds it.
                    run = body["run"].take();
                }
                Ok((status, _)) if status.is_client_error() => bail!("HTTP {status} on {path}"),
                failed => {
                    failures += 1;
                    if failures >= patience {
                        let why = failed.map_or_else(|e| e.to_string(), |(s, _)| s.to_string());
                        bail!("run {run_id} still unknown after {failures} polls: {why}");
                    }
                }
            }
        }
    }
    println!("{}", serde_json::to_string_pretty(&run)?);
    match run_end(&run) {
        Some(Err(why)) if wait => bail!("run did not apply: {why}"),
        _ => Ok(()),
    }
}

fn run() -> Result<()> {
    let ops = ops();
    let matches = cli(&ops).get_matches();
    let http = Client::builder()
        .user_agent(concat!("pnex-cli/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let org = matches.get_one::<i64>("org").copied();
    match matches.subcommand().expect("subcommand required") {
        ("login", m) => {
            let session = session::login(&http, m.get_one::<String>("url").expect("required"))?;
            session.save()?;
            eprintln!("Signed in to {}.", session.url);
            run_org(&http, org)
        }
        ("logout", _) => {
            if Session::logout()? {
                eprintln!("Session forgotten.");
            }
            Ok(())
        }
        ("org", m) => run_org(&http, m.get_one::<i64>("id").copied()),
        ("raw", m) => {
            let method = Method::from_bytes(
                m.get_one::<String>("method")
                    .expect("required")
                    .to_uppercase()
                    .as_bytes(),
            )?;
            if method != Method::GET && !m.get_flag("yes") {
                bail!("refused without --yes: raw {method}");
            }
            let mut session = Session::load()?;
            let url = reqwest::Url::parse(&format!(
                "{}{}",
                session.url,
                m.get_one::<String>("path").expect("required")
            ))?;
            let body = m
                .get_one::<String>("body")
                .map(|b| read_body(b).map(|bytes| ("application/json".to_string(), bytes)))
                .transpose()?;
            let org = org.or(session.org);
            send(&http, &mut session, org, method, url, vec![], body)
        }
        ("dataset", m) => {
            let (_, m) = m.subcommand().expect("subcommand required");
            if let Some(reason) = danger("datasets_uploads_start", &Method::POST) {
                if !m.get_flag("yes") {
                    bail!("refused without --yes: {reason}");
                }
            }
            let mut session = Session::load()?;
            let org = org.or(session.org);
            if org.is_none() {
                bail!("no organisation: pick one with `pnex org <id>` or pass --org");
            }
            load_dataset(
                &http,
                &mut session,
                org,
                m.get_one::<String>("id").expect("required"),
                m.get_one::<String>("file").expect("required"),
                m.get_one::<String>("delimiter"),
                m.get_flag("append"),
                // Five minutes: longer than a rollout.
                150,
            )
        }
        ("import", m) => {
            let (_, m) = m.subcommand().expect("subcommand required");
            let start = "ontology_imports_runs_start";
            if let Some(reason) = danger(start, &Method::POST) {
                if !m.get_flag("yes") {
                    bail!("refused without --yes: {reason}");
                }
            }
            let mut session = Session::load()?;
            let org = org.or(session.org);
            if org.is_none() {
                bail!("no organisation: pick one with `pnex org <id>` or pass --org");
            }
            run_import(
                &http,
                &mut session,
                org,
                m.get_one::<String>("id").expect("required"),
                m.get_flag("wait"),
                std::time::Duration::from_secs(2),
                // Five minutes: longer than a rollout.
                150,
            )
        }
        (tag, m) => {
            let (action, m) = m.subcommand().expect("subcommand required");
            let op = ops
                .iter()
                .find(|o| o.tag == tag && o.action == action)
                .expect("known command");
            run_op(&http, op, m, org)
        }
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("pnex: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_file_is_cut_in_parts_with_nothing_lost() {
        let size = UPLOAD_PART_BYTES as usize + 5;
        let mut file = std::io::Cursor::new(vec![7u8; size]);
        assert_eq!(
            read_part(&mut file).unwrap().len() as u64,
            UPLOAD_PART_BYTES
        );
        assert_eq!(read_part(&mut file).unwrap().len(), 5);
        assert!(read_part(&mut file).unwrap().is_empty());
    }

    use super::*;

    #[test]
    fn every_operation_is_a_valid_command() {
        let ops = ops();
        assert!(ops.len() > 300);
        cli(&ops).debug_assert();
    }

    #[test]
    fn a_run_ends_on_anything_but_running() {
        let end = |v: Value| run_end(&v);
        assert_eq!(end(serde_json::json!({"state": "running"})), None);
        assert_eq!(end(serde_json::json!({"state": "applied"})), Some(Ok(())));
        assert_eq!(
            end(serde_json::json!({"state": "failed", "state_code": "import-run-interrupted"})),
            Some(Err("failed: import-run-interrupted".into()))
        );
        // A run stopped at a preview wrote nothing: not a success.
        assert_eq!(
            end(serde_json::json!({"state": "preview"})),
            Some(Err("preview".into()))
        );
    }

    /// Answers one canned response per connection, in order, and records the request lines.
    fn mock(answers: Vec<(u16, &'static str)>) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (status, body) in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let _ = tx.send(line.trim().to_string());
                let mut header = String::new();
                while reader.read_line(&mut header).unwrap() > 2 {
                    header.clear();
                }
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (url, rx)
    }

    fn session(url: &str) -> Session {
        serde_json::from_value(serde_json::json!({
            "url": url, "org": 7, "access_token": "t", "refresh_token": "r",
            "expires_at": u64::MAX,
        }))
        .unwrap()
    }

    #[test]
    fn wait_follows_the_run_through_a_replaced_pod_and_reports_its_end() {
        let http = Client::new();
        let tick = std::time::Duration::from_millis(1);
        let (url, requests) = mock(vec![
            (202, r#"{"id": "r1", "state": "running"}"#),
            (503, r#"{"error": "server-busy"}"#),
            (200, r#"{"run": {"id": "r1", "state": "running"}}"#),
            (
                200,
                r#"{"run": {"id": "r1", "state": "failed", "state_code": "import-schema-changed"}}"#,
            ),
        ]);
        let e = run_import(&http, &mut session(&url), Some(7), "i1", true, tick, 3).unwrap_err();
        assert_eq!(
            e.to_string(),
            "run did not apply: failed: import-schema-changed"
        );
        let lines: Vec<String> = requests.try_iter().collect();
        assert_eq!(lines[0], "POST /api/v1/ontology/imports/i1/runs HTTP/1.1");
        assert_eq!(lines[3], "GET /api/v1/ontology/imports/i1/runs/r1 HTTP/1.1");

        let (url, _) = mock(vec![
            (202, r#"{"id": "r2", "state": "running"}"#),
            (200, r#"{"run": {"id": "r2", "state": "applied"}}"#),
        ]);
        run_import(&http, &mut session(&url), Some(7), "i1", true, tick, 3).unwrap();

        // Not waiting: the started run is enough.
        let (url, _) = mock(vec![(202, r#"{"id": "r3", "state": "running"}"#)]);
        run_import(&http, &mut session(&url), Some(7), "i1", false, tick, 3).unwrap();

        // A server that never comes back ends the wait instead of hanging.
        let (url, _) = mock(vec![
            (202, r#"{"id": "r4", "state": "running"}"#),
            (503, "{}"),
            (503, "{}"),
        ]);
        let e = run_import(&http, &mut session(&url), Some(7), "i1", true, tick, 2).unwrap_err();
        assert!(e.to_string().contains("still unknown after 2 polls"), "{e}");
    }

    #[test]
    fn dangerous_reasons_exist() {
        for d in DANGEROUS {
            assert!(
                EXCLUDED.iter().any(|(r, _)| r.split(':').next() == Some(d)),
                "DANGEROUS lists `{d}`, no EXCLUDED reason has it"
            );
        }
    }

    #[test]
    fn danger_classification() {
        let ops = ops();
        let by_id = |tag: &str, action: &str| {
            ops.iter()
                .find(|o| o.tag == tag && o.action == action)
                .unwrap()
        };
        assert!(by_id("devices", "delete").danger.is_some());
        assert!(by_id("devices", "list").danger.is_none());
        assert!(by_id("flows", "list").danger.is_none());
        for op in ops.iter().filter(|o| o.method == Method::DELETE) {
            assert!(
                op.danger.is_some(),
                "{} {} deletes without --yes",
                op.tag,
                op.action
            );
        }
    }
}
