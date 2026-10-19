//! Observation of the engine next to the R2 guards (iam.md lot I1).
//!
//! The middleware resolves the operation of the request (method + matched
//! route → operationId of the spec) and leaves a [`Slot`] in the request;
//! the `OrgContext` extractor records the engine decision in it. Once the
//! handler has answered, both are compared:
//! - engine denies, handler succeeds → the registry is too strict;
//! - engine allows, handler answers a role 403 → the registry is too lax;
//! - an `OrgContext` on an operation the registry does not mark `Org`.
//!
//! Divergences are logged (`target: "iam"`). In tests, the existing suites
//! prove the equivalence of the managed roles (D287): with
//! `PNEX_IAM_OBSERVE=strict` the answer becomes a 500 naming the
//! divergence, and `PNEX_IAM_OBSERVE_LOG=<file>` appends one line each.

use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::{MatchedPath, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use pnex_api_contract::iam::Access;

use super::Decision;
use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;

/// 403 codes that are not a role guard: field-level gates (binding a vault
/// secret), ownership (the author of a comment) and the per-type ontology
/// write role (D188, folded into roles at lot I3). They stay in handlers.
const FIELD_GATES: &[&str] = &[
    pnex_core::err_codes::SECRET_WRITE_FORBIDDEN,
    pnex_core::err_codes::ONTOLOGY_WRITE_FORBIDDEN,
];

/// What the extractor saw for the current request.
#[derive(Clone, Debug)]
struct Observed {
    role: OrgMemberRole,
    decision: Option<Decision>,
    who: Who,
}

/// Who called, for the audit (D294).
#[derive(Clone, Debug)]
pub struct Who {
    pub org_id: i64,
    pub user_id: i64,
    pub service_account_id: Option<uuid::Uuid>,
}

/// Registry entry of the current request, and the engine decision once
/// the `OrgContext` extractor ran.
#[derive(Clone)]
pub struct Slot {
    seen: Arc<Mutex<Option<Observed>>>,
}

impl Slot {
    /// Called by the `OrgContext` extractor, before it refuses, with the
    /// caller and the engine decision (`None` off the registry's org
    /// operations).
    pub fn record(&self, role: &OrgMemberRole, decision: Option<Decision>, who: Who) {
        if let Ok(mut seen) = self.seen.lock() {
            *seen = Some(Observed {
                role: *role,
                decision,
                who,
            });
        }
    }
}

fn strict() -> bool {
    static STRICT: OnceLock<bool> = OnceLock::new();
    *STRICT.get_or_init(|| std::env::var("PNEX_IAM_OBSERVE").as_deref() == Ok("strict"))
}

pub async fn middleware(mut req: Request, next: Next) -> Response {
    let entry = super::operation_of(req.method(), req.extensions().get::<MatchedPath>());
    let Some((operation, access)) = entry else {
        return next.run(req).await;
    };
    let slot = Slot {
        seen: Arc::default(),
    };
    req.extensions_mut().insert(slot.clone());
    let res = next.run(req).await;
    let Some(seen) = slot.seen.lock().ok().and_then(|mut s| s.take()) else {
        return res;
    };
    let status = res.status();
    audit(operation, access, &seen, status);
    let (res, divergence) = match &seen.decision {
        None => (res, Some("org context on an operation not marked Org")),
        Some(d) if !d.allowed() && status.is_success() => {
            (res, Some("engine denies, handler allowed"))
        }
        Some(d) if d.allowed() && status == StatusCode::FORBIDDEN => {
            let (res, code) = error_code(res).await;
            let role_guard = code.as_deref().is_some_and(|c| {
                (c.ends_with("-forbidden") || c.ends_with("-admin-required"))
                    && !FIELD_GATES.contains(&c)
            });
            (res, role_guard.then_some("engine allows, handler denied"))
        }
        Some(_) => (res, None),
    };
    let Some(divergence) = divergence else {
        return res;
    };
    let permission = match access {
        Access::Org(p) => p,
        _ => "-",
    };
    tracing::warn!(
        target: "iam",
        operation,
        permission,
        role = ?seen.role,
        status = status.as_u16(),
        "iam divergence: {divergence}"
    );
    if let Some(path) = std::env::var_os("PNEX_IAM_OBSERVE_LOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(
                f,
                "{operation}\t{permission}\t{:?}\t{status}\t{divergence}",
                seen.role
            );
        }
    }
    if strict() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "iam divergence on {operation} ({permission}, {:?}, handler {status}): {divergence}",
                seen.role
            ),
        )
            .into_response();
    }
    res
}

/// D294: refusals, successful access management writes and every call
/// made with an access key go to the `iam_decisions` journal.
fn audit(operation: &'static str, access: Access, seen: &Observed, status: StatusCode) {
    let (Access::Org(permission), Some(decision)) = (access, &seen.decision) else {
        return;
    };
    let iam_write = permission.starts_with("iam:") && !permission.ends_with(":read");
    let key = seen.who.service_account_id.is_some();
    if decision.allowed() && !key && !(iam_write && status.is_success()) {
        return;
    }
    super::audit::submit(super::audit::Entry {
        org_id: seen.who.org_id,
        user_id: seen.who.user_id,
        service_account_id: seen.who.service_account_id,
        operation: operation.to_string(),
        permission: permission.to_string(),
        allowed: decision.allowed(),
        status: status.as_u16(),
        origin: if key { "access-key" } else { "ui" },
    });
}

/// Machine code of an `ErrorDetail` answer; the body is rebuilt as is.
async fn error_code(res: Response) -> (Response, Option<String>) {
    let (parts, body) = res.into_parts();
    let bytes = axum::body::to_bytes(body, 64 * 1024)
        .await
        .unwrap_or_default();
    let code = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string));
    (Response::from_parts(parts, bytes.into()), code)
}
