use super::*;

// ─────────────────────────────── Templates ───────────────────────────────

#[derive(Debug, Deserialize)]
pub(super) struct TemplateInput {
    name: String,
    #[serde(default)]
    subject: Option<String>,
    body: String,
    #[serde(default)]
    vars: Vec<TemplateVar>,
}

fn validate_template_input(name: &str, body: &str, subject: Option<&str>) -> Option<Response> {
    let name = name.trim();
    if name.is_empty() {
        return Some(field_status("name", err_codes::FIELD_REQUIRED));
    }
    if name.chars().count() > 200 {
        return Some(field_status(
            "name",
            &format!("{}:200", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    if body.trim().is_empty() {
        return Some(field_status("body", err_codes::FIELD_REQUIRED));
    }
    // Syntaxe minijinja : rendu à vide pour refuser un template cassé à la
    // sauvegarde (l'erreur au send serait bien plus difficile à déboguer).
    if let Err(e) = pnex_notify::render(
        subject,
        body,
        &Default::default(),
        serde_json::json!({}),
        serde_json::json!({}),
    ) {
        if matches!(e, pnex_notify::NotifyError::Render(_)) {
            return Some(template_render_error(&e).into_response());
        }
        // TooLarge sur contextes vides : impossible — toute autre erreur non.
    }
    None
}

/// Fusionne les vars détectées dans le template (`{{ name }}` /
/// `{{ vars.name }}` du sujet et du corps, cf. `pnex_notify::template_vars`)
/// avec celles déclarées par l'UI : une var détectée manquante est ajoutée
/// (example vide), l'example d'une var déjà déclarée est conservé. L'ordre
/// UI d'abord, puis les détectées manquantes dans l'ordre d'apparition —
/// cet ordre devient l'ordre des ancres canvas du nœud notify.
fn merge_template_vars(detected: Vec<String>, declared: Vec<TemplateVar>) -> Vec<TemplateVar> {
    let mut out = declared;
    for name in detected {
        if !out.iter().any(|v| v.name == name) {
            out.push(TemplateVar {
                name,
                example: String::new(),
            });
        }
    }
    out
}

async fn ensure_template_name_free(db: &DatabaseConnection, org_id: i64, name: &str) -> Result<()> {
    let clash = NotifyTemplates::find()
        .filter(notify_templates::Column::OrgId.eq(org_id))
        .filter(notify_templates::Column::Name.eq(name))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some();
    if clash {
        return Err(conflict(
            "notify-template-name-conflict",
            "A template already uses this name.",
        ));
    }
    Ok(())
}

/// `GET /api/v1/notify/templates`.
pub(super) async fn list_templates(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = NotifyTemplates::find()
        .filter(notify_templates::Column::OrgId.eq(org.org.id))
        .order_by_desc(notify_templates::Column::Id)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<NotifyTemplate> = rows.into_iter().map(template_dto).collect();
    let count = results.len() as i64;
    let (skip, take) = page.slice(results.len());
    let results: Vec<NotifyTemplate> = results.into_iter().skip(skip).take(take).collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/notify/templates",
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

/// `GET /api/v1/notify/templates/{id}`.
pub(super) async fn template_detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(m) = find_template(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    Ok(format::json(template_dto(m)).into_response())
}

/// `POST /api/v1/notify/templates`.
pub(super) async fn create_template(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<TemplateInput>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "notify-write-forbidden",
            "Owner, admin or member role required to manage notifications.",
        ));
    }
    let name = params.name.trim().to_string();
    if let Some(resp) = validate_template_input(&name, &params.body, params.subject.as_deref()) {
        return Ok(resp);
    }
    ensure_template_name_free(&ctx.db, org.org.id, &name).await?;
    let vars = merge_template_vars(
        pnex_notify::template_vars(params.subject.as_deref(), &params.body),
        params.vars,
    );
    let m = notify_templates::ActiveModel {
        org_id: Set(org.org.id),
        name: Set(name),
        subject: Set(params.subject),
        body: Set(params.body),
        vars: Set(parse_vars(&vars)?),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    Ok((StatusCode::CREATED, format::json(template_dto(m))).into_response())
}

/// `PUT /api/v1/notify/templates/{id}`.
pub(super) async fn update_template(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(params): Json<TemplateInput>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "notify-write-forbidden",
            "Owner, admin or member role required to manage notifications.",
        ));
    }
    let Some(m) = find_template(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let name = params.name.trim().to_string();
    if let Some(resp) = validate_template_input(&name, &params.body, params.subject.as_deref()) {
        return Ok(resp);
    }
    if name != m.name {
        ensure_template_name_free(&ctx.db, org.org.id, &name).await?;
    }
    let vars = merge_template_vars(
        pnex_notify::template_vars(params.subject.as_deref(), &params.body),
        params.vars,
    );
    let mut active: notify_templates::ActiveModel = m.into();
    active.name = Set(name);
    active.subject = Set(params.subject);
    active.body = Set(params.body);
    active.vars = Set(parse_vars(&vars)?);
    let updated = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(format::json(template_dto(updated)).into_response())
}

/// `DELETE /api/v1/notify/templates/{id}` — 204 ; les flows déployés
/// gardent leur snapshot (re-déployer pour propager, école « save ≠
/// déployé ») et le journal reste immuable (template_id sans FK).
pub(super) async fn delete_template(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "notify-write-forbidden",
            "Owner, admin or member role required to manage notifications.",
        ));
    }
    let Some(m) = find_template(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    m.into_active_model()
        .delete(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct PreviewBody {
    #[serde(default)]
    vars: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    payload: serde_json::Value,
}

/// `POST /api/v1/notify/templates/{id}/preview` — rendu seul, **jamais
/// d'envoi** ; les vars manquantes prennent leur `example` déclaré (D52).
/// Body en `Bytes` parsé à la main (même école que le test de canal : un
/// `Option<Json<T>>` casse sur les headers Content-Type injectés/dupliqués
/// des clients de test).
pub(super) async fn preview_template(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    raw: axum::body::Bytes,
) -> Result<Response> {
    let Some(m) = find_template(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let PreviewBody { vars, payload } = if raw.is_empty() {
        PreviewBody::default()
    } else {
        serde_json::from_slice(&raw).map_err(|e| {
            Error::CustomError(
                StatusCode::BAD_REQUEST,
                loco_rs::controller::ErrorDetail::new(
                    err_codes::NOTIFY_TEST_BODY_INVALID,
                    format!("Invalid JSON body: {e}"),
                ),
            )
        })?
    };
    // Un example **vide** n'est pas injecté : une var déclarée vide
    // masquerait la clé payload de même nom au niveau racine du rendu
    // (vars > payload dans le contexte — bug du preview « [] alerte »).
    let declared: Vec<TemplateVar> = serde_json::from_value(m.vars.clone()).unwrap_or_default();
    let mut vars = vars;
    for v in declared {
        if !v.example.is_empty() {
            vars.entry(v.name).or_insert(v.example);
        }
    }
    let rendered = pnex_notify::render(
        m.subject.as_deref(),
        &m.body,
        &vars,
        payload,
        serde_json::json!({ "preview": true }),
    )
    .map_err(|e| template_render_error(&e))?;
    Ok(format::json(pnex_core::PreviewResult {
        subject: rendered.subject,
        body: rendered.body,
    })
    .into_response())
}
