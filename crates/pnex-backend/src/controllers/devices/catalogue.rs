use super::*;

// ─────────────────────── Catalogue global ───────────────────────

#[derive(Debug, Default, Deserialize)]
pub(super) struct CapabilityQuery {
    /// input | output | input_output (autre valeur → liste vide).
    mode: Option<String>,
    /// Recherche OU sur le nom.
    search: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/device-capabilities` — catalogue, authentifié. Table de
/// référence bornée : le filtre `mode` reste en Rust, puis découpage.
pub(super) async fn capabilities(
    State(ctx): State<AppContext>,
    _auth: AuthUser,
    Query(q): Query<CapabilityQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = device_capabilities::Entity::find()
        .order_by_asc(device_capabilities::Column::Id)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let caps: Vec<pnex_core::DeviceCapability> = rows
        .into_iter()
        .filter(|c| {
            q.mode
                .as_deref()
                .is_none_or(|m| capability_mode_str(c.mode) == m)
                && pagination::rust_search_match(&q.search, &[c.name.as_str()])
        })
        .map(|c| pnex_core::DeviceCapability {
            id: c.id,
            name: c.name,
            mode: capability_mode_str(c.mode).to_string(),
        })
        .collect();
    let count = caps.len() as i64;
    let (skip, take) = page.slice(caps.len());
    let mut filters = Vec::new();
    if let Some(m) = &q.mode {
        filters.push(("mode".to_string(), m.clone()));
    }
    if let Some(s) = &q.search {
        filters.push(("search".to_string(), s.clone()));
    }
    format::json(pagination::envelope(
        "/api/v1/device-capabilities",
        &filters,
        page,
        count,
        caps.into_iter().skip(skip).take(take).collect(),
    ))
}

/// Query-string en map multi-valeurs (`capabilities=a&capabilities=b`).
fn raw_query_map(raw: Option<&str>) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(q) = raw {
        for (k, v) in form_urlencoded::parse(q.as_bytes()) {
            map.entry(k.into_owned()).or_default().push(v.into_owned());
        }
    }
    map
}

/// `GET /api/v1/predefined-devices` — catalogue global, authentifié,
/// **paginé en SQL** (D14) : count + LIMIT/OFFSET côté base, hydration
/// (noms type/board, capacités) limitée à la seule page renvoyée.
///
/// Filtres : `capabilities` (répétable, OU — sous-requête M2M), `board`,
/// `device_type`, `name`/`pretty_name` (icontains via ILIKE), `revision`
/// (exact), `search` (OU multi-champs : nom, pretty, description, type,
/// board, capacités — ILIKE).
pub(super) async fn predefined_list(
    State(ctx): State<AppContext>,
    _auth: AuthUser,
    RawQuery(raw): RawQuery,
) -> Result<Response> {
    let params = raw_query_map(raw.as_deref());
    let first = |k: &str| params.get(k).and_then(|v| v.first().cloned());
    let caps_filter = params.get("capabilities").cloned().unwrap_or_default();
    let (board_f, type_f, name_f, pretty_f, rev_f, search_f) = (
        first("board"),
        first("device_type"),
        first("name"),
        first("pretty_name"),
        first("revision"),
        first("search"),
    );
    let page = pagination::PageParams::from_map(&params);

    // Liens next/previous : rejouer les filtres actifs.
    let mut filters: Vec<(String, String)> = caps_filter
        .iter()
        .map(|c| ("capabilities".to_string(), c.clone()))
        .collect();
    for (key, value) in [
        ("board", &board_f),
        ("device_type", &type_f),
        ("name", &name_f),
        ("pretty_name", &pretty_f),
        ("revision", &rev_f),
        ("search", &search_f),
    ] {
        if let Some(v) = value {
            filters.push((key.to_string(), v.clone()));
        }
    }

    // Résolution des filtres par nom → id (type, board). Nom inconnu →
    // liste vide cohérente avec le count.
    let empty_page = |page: pagination::PageParams, filters: &[(String, String)]| {
        format::json(pagination::envelope(
            "/api/v1/predefined-devices",
            filters,
            page,
            0,
            Vec::<pnex_core::PredefinedDevice>::new(),
        ))
    };
    let type_id = match type_f.as_deref() {
        Some(name) => device_types::Entity::find()
            .filter(device_types::Column::Name.eq(name))
            .one(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .map(|t| t.id),
        None => None,
    };
    if type_f.is_some() && type_id.is_none() {
        return empty_page(page, &filters);
    }
    let board_id = match board_f.as_deref() {
        Some(name) => mcu_boards::Entity::find()
            .filter(mcu_boards::Column::Name.eq(name))
            .one(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .map(|b| b.id),
        None => None,
    };
    if board_f.is_some() && board_id.is_none() {
        return empty_page(page, &filters);
    }

    let mut query = predefined_devices::Entity::find();
    if let Some(id) = type_id {
        query = query.filter(predefined_devices::Column::DeviceTypeId.eq(id));
    }
    if let Some(id) = board_id {
        query = query.filter(predefined_devices::Column::BoardId.eq(id));
    }
    if let Some(n) = &name_f {
        query = query.filter(
            Expr::col((predefined_devices::Entity, predefined_devices::Column::Name))
                .ilike(format!("%{n}%")),
        );
    }
    if let Some(p) = &pretty_f {
        query = query.filter(
            Expr::col((
                predefined_devices::Entity,
                predefined_devices::Column::PrettyName,
            ))
            .ilike(format!("%{p}%")),
        );
    }
    if let Some(r) = &rev_f {
        query = query.filter(predefined_devices::Column::Revision.eq(r));
    }
    if !caps_filter.is_empty() {
        // OU sur la M2M : id IN (sous-requête des liens vers les caps visées).
        let sub = predefined_device_capabilities::Entity::find()
            .left_join(device_capabilities::Entity)
            .filter(device_capabilities::Column::Name.is_in(caps_filter))
            .select_only()
            .column(predefined_device_capabilities::Column::PredefinedDeviceId);
        query = query.filter(predefined_devices::Column::Id.in_subquery(sub.into_query()));
    }
    if let Some(s) = search_f.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        // Recherche OU multi-champs, poussée en SQL (ILIKE PG = insensible à
        // la casse) pour rester compatible avec le LIMIT/OFFSET base.
        let pat = format!("%{s}%");
        let text_or_refs = sea_orm::Condition::any()
            .add(
                Expr::col((predefined_devices::Entity, predefined_devices::Column::Name))
                    .ilike(pat.clone()),
            )
            .add(
                Expr::col((
                    predefined_devices::Entity,
                    predefined_devices::Column::PrettyName,
                ))
                .ilike(pat.clone()),
            )
            .add(
                Expr::col((
                    predefined_devices::Entity,
                    predefined_devices::Column::Description,
                ))
                .ilike(pat.clone()),
            )
            .add(
                predefined_devices::Column::DeviceTypeId.in_subquery(
                    device_types::Entity::find()
                        .filter(
                            Expr::col((device_types::Entity, device_types::Column::Name))
                                .ilike(pat.clone()),
                        )
                        .select_only()
                        .column(device_types::Column::Id)
                        .into_query(),
                ),
            )
            .add(
                predefined_devices::Column::BoardId.in_subquery(
                    mcu_boards::Entity::find()
                        .filter(
                            Expr::col((mcu_boards::Entity, mcu_boards::Column::Name))
                                .ilike(pat.clone()),
                        )
                        .select_only()
                        .column(mcu_boards::Column::Id)
                        .into_query(),
                ),
            )
            .add(
                predefined_devices::Column::Id.in_subquery(
                    predefined_device_capabilities::Entity::find()
                        .left_join(device_capabilities::Entity)
                        .filter(
                            Expr::col((
                                device_capabilities::Entity,
                                device_capabilities::Column::Name,
                            ))
                            .ilike(pat),
                        )
                        .select_only()
                        .column(predefined_device_capabilities::Column::PredefinedDeviceId)
                        .into_query(),
                ),
            );
        query = query.filter(text_or_refs);
    }

    let count = query
        .clone()
        .count(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)? as i64;
    let rows = query
        .order_by_asc(predefined_devices::Column::Id)
        .offset(page.offset as u64)
        .limit(page.limit as u64)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Hydration de la page uniquement.
    let type_names: HashMap<i64, String> = device_types::Entity::find()
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();
    let board_names: HashMap<i64, String> = mcu_boards::Entity::find()
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|b| (b.id, b.name))
        .collect();
    let caps = capabilities_of(&ctx.db, &rows.iter().map(|p| p.id).collect::<Vec<_>>()).await?;

    let out: Vec<pnex_core::PredefinedDevice> = rows
        .into_iter()
        .map(|pd| {
            let screen_kind = screen_capability(&pd);
            pnex_core::PredefinedDevice {
                name: pd.name,
                pretty_name: pd.pretty_name,
                prestashop_product_id: pd.prestashop_product_id,
                prestashop_buy_url: pd.prestashop_buy_url,
                byod_doc_url: pd.byod_doc_url,
                image_source_url: pd.image_source_url,
                description: pd.description,
                description_i18n: pd.description_i18n,
                revision: pd.revision,
                device_type: type_names
                    .get(&pd.device_type_id)
                    .cloned()
                    .unwrap_or_default(),
                capabilities: caps
                    .get(&pd.id)
                    .map(|list| list.iter().map(|c| c.name.clone()).collect())
                    .unwrap_or_default(),
                board: board_names.get(&pd.board_id).cloned().unwrap_or_default(),
                has_screen: screen_kind.is_some(),
                screen_kind,
            }
        })
        .collect();
    format::json(pagination::envelope(
        "/api/v1/predefined-devices",
        &filters,
        page,
        count,
        out,
    ))
}
