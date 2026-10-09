//! Pure model layer of the POI tree: drag payloads, containment tree
//! snapshot and the row-view builder (everything resolved before the rsx).

use super::*;

/// Cible de dépôt : racine du POI ou un dossier.
#[derive(Clone, PartialEq)]
pub(super) enum DropId {
    Root,
    Folder(String),
}

/// Objet saisi par la poignée — tout ce qu'il faut pour muter au dépôt.
#[derive(Clone)]
pub(super) struct DragItem {
    pub(super) kind: String,
    pub(super) id: String,
    /// Nom d'affichage courant (recrée le label de l'arête au retour racine).
    pub(super) label: String,
    /// Arête `placed_on` d'un objet racine (supprimée si rangé en dossier).
    pub(super) edge_id: Option<i64>,
    /// Emplacement courant — un dépôt identique est un no-op.
    pub(super) from: DropId,
}

/// Payload du bouton Detach.
#[derive(Clone)]
pub(super) enum Detach {
    /// Objet racine : l'arête `placed_on` part.
    Edge(i64),
    /// Objet rangé : éjection vers la racine du POI (containment détaché +
    /// arête `placed_on` recréée — un détachement pur laissait l'objet
    /// orphelin, invisible de tous les arbres, constat 2026-09-13). Un device
    /// rangé n'est que détaché du dossier (placement intact, amendement D43).
    Contained { kind: String, id: String },
    /// Device racine : le placement part — le device redevient libre
    /// (amendement D43, 2026-09-13).
    Placement(i64),
}

/// État de l'arbre du POI (marche containment, noms hydratés par le serveur).
#[derive(Clone, Default, PartialEq)]
pub(super) struct TreeModel {
    /// Dossiers enfants du POI (ordre serveur).
    pub(super) root_folders: Vec<RefNode>,
    /// folder id → enfants (ordre serveur).
    pub(super) children: HashMap<String, Vec<RefNode>>,
    /// Tout ce qui vit sous ≥ 1 dossier (exclu des racines du rendu).
    pub(super) contained: HashSet<(String, String)>,
}

// ─────────────────────────── helpers graphe ───────────────────────────

/// Descendants (tous kinds) d'un dossier — garde anti-cycle du drag.
pub(super) fn descendants_of(
    children: &HashMap<String, Vec<RefNode>>,
    root_id: &str,
) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    let mut stack = vec![root_id.to_string()];
    while let Some(cur) = stack.pop() {
        if let Some(kids) = children.get(&cur) {
            for c in kids {
                if out.insert(c.id.clone()) && c.kind == "folder" {
                    stack.push(c.id.clone());
                }
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)] // mirrors the tree data sources one-to-one
pub(super) fn build_views(
    model: &TreeModel,
    links: &[VizLink],
    placements: &[DevicePlacement],
    devices: &[(String, String, String)],
    expanded: &HashSet<String>,
    dragging: &Option<DragItem>,
    preview: &Option<PreviewTarget>,
    pinned: &Option<PreviewTarget>,
) -> Vec<TreeRow> {
    let mut device_by_pk: HashMap<String, (String, String)> = HashMap::new();
    let mut slug_index: HashMap<String, (String, String)> = HashMap::new();
    for (pk, slug, dtype) in devices {
        device_by_pk.insert(pk.clone(), (slug.clone(), dtype.clone()));
        slug_index.insert(slug.clone(), (pk.clone(), dtype.clone()));
    }
    let mut placement_by_slug: HashMap<String, DevicePlacement> = HashMap::new();
    for p in placements {
        placement_by_slug.insert(p.device_id.clone(), p.clone());
    }
    let ctx = ViewCtx {
        model,
        expanded,
        dragging,
        device_by_pk,
        placement_by_slug,
        preview,
        pinned,
    };

    let mut rows = Vec::new();
    // Ordre serveur conservé (tri par sort_key puis kind/id).
    for folder in &model.root_folders {
        push_folder(&ctx, folder, 0, DropId::Root, &mut rows);
    }

    // Objet racine = arête `placed_on` non rangée (liens morts D26 compris,
    // toujours racine).
    for link in links {
        if model
            .contained
            .contains(&(link.target_kind.clone(), link.target_id.clone()))
        {
            continue;
        }
        let name = link
            .target_label
            .clone()
            .unwrap_or_else(|| link.target_id.clone());
        // Objet aperçuable (média/tour/dashboard non mort) : clic → panneau.
        let target = (!link.target_dead).then(|| PreviewTarget {
            kind: link.target_kind.clone(),
            id: link.target_id.clone(),
            name: name.clone(),
        });
        rows.push(TreeRow {
            key: format!("edge-{}", link.id),
            depth: 0,
            is_folder: false,
            expanded_now: false,
            has_children: false,
            emoji: None,
            icon_kind: Some(link.target_kind.clone()),
            subtitle: None,
            is_device: false,
            is_dead: link.target_dead,
            is_dragged: dragging
                .as_ref()
                .map(|d| d.kind == link.target_kind && d.id == link.target_id)
                .unwrap_or(false),
            toggle: None,
            drag: Some(DragItem {
                kind: link.target_kind.clone(),
                id: link.target_id.clone(),
                label: name.clone(),
                edge_id: Some(link.id),
                from: DropId::Root,
            }),
            drop_id: None,
            is_previewed: ctx
                .preview
                .as_ref()
                .zip(target.as_ref())
                .is_some_and(|(p, t)| p.kind == t.kind && p.id == t.id),
            is_pinned: ctx
                .pinned
                .as_ref()
                .zip(target.as_ref())
                .is_some_and(|(p, t)| p.kind == t.kind && p.id == t.id),
            preview: target.clone(),
            pin_target: target.clone(),
            clear_target: target,
            detach: Some(Detach::Edge(link.id)),
            delete: None,
            labels: None,
            placement: None,
            name,
        });
    }

    // Device racine = placement D43 non rangé — résolu via l'index local
    // slug→(pk, type) construit depuis la liste devices du drawer.
    for p in placements {
        let Some((pk, dtype)) = slug_index.get(&p.device_id) else {
            continue;
        };
        if model
            .contained
            .contains(&("device".to_string(), pk.clone()))
        {
            continue;
        }
        rows.push(TreeRow {
            key: format!("placement-{}", p.id),
            depth: 0,
            is_folder: false,
            expanded_now: false,
            has_children: false,
            emoji: None,
            icon_kind: Some("device".to_string()),
            subtitle: Some(dtype.clone()),
            is_device: true,
            is_dead: false,
            is_dragged: dragging
                .as_ref()
                .map(|d| d.kind == "device" && &d.id == pk)
                .unwrap_or(false),
            toggle: None,
            drag: Some(DragItem {
                kind: "device".to_string(),
                id: pk.clone(),
                label: p.device_id.clone(),
                edge_id: None,
                from: DropId::Root,
            }),
            drop_id: None,
            is_previewed: false,
            is_pinned: false,
            preview: None,
            pin_target: None,
            clear_target: None,
            detach: Some(Detach::Placement(p.id)),
            delete: None,
            labels: None,
            placement: Some(p.clone()),
            name: p.device_id.clone(),
        });
    }
    rows
}

/// Pousser un dossier puis ses enfants (récursif, branches dépliées).
fn push_folder(
    ctx: &ViewCtx,
    folder: &RefNode,
    depth: usize,
    from: DropId,
    rows: &mut Vec<TreeRow>,
) {
    let fid = folder.id.clone();
    let name = folder
        .name
        .clone()
        .unwrap_or_else(|| format!("folder #{}", fid));
    let fid_num = fid.parse::<i64>().unwrap_or(0);
    let expanded_now = ctx.expanded.contains(&fid);
    let children = ctx.model.children.get(&fid).cloned().unwrap_or_default();
    rows.push(TreeRow {
        key: format!("folder-{fid}"),
        depth,
        is_folder: true,
        expanded_now,
        has_children: !children.is_empty(),
        emoji: folder.emoji.clone(),
        icon_kind: None,
        subtitle: None,
        is_device: false,
        is_dead: false,
        is_dragged: ctx
            .dragging
            .as_ref()
            .map(|d| d.kind == "folder" && d.id == fid)
            .unwrap_or(false),
        toggle: Some(fid.clone()),
        drag: Some(DragItem {
            kind: "folder".to_string(),
            id: fid.clone(),
            label: name.clone(),
            edge_id: None,
            from,
        }),
        drop_id: Some(DropId::Folder(fid.clone())),
        is_previewed: false,
        is_pinned: false,
        preview: None,
        pin_target: None,
        clear_target: None,
        detach: None,
        delete: Some((fid_num, name.clone())),
        labels: Some((fid_num, name.clone())),
        placement: None,
        name,
    });
    if !expanded_now {
        return;
    }
    // Dossiers d'abord (présentation), ordre serveur dans chaque groupe.
    let sub_folders: Vec<&RefNode> = children.iter().filter(|c| c.kind == "folder").collect();
    let others: Vec<&RefNode> = children.iter().filter(|c| c.kind != "folder").collect();
    for sub in sub_folders {
        push_folder(ctx, sub, depth + 1, DropId::Folder(fid.clone()), rows);
    }
    for child in others {
        let (name, subtitle, is_device, placement) = if child.kind == "device" {
            let (slug, dtype) = ctx
                .device_by_pk
                .get(&child.id)
                .cloned()
                .unwrap_or_else(|| (format!("#{}", child.id), String::new()));
            (
                slug.clone(),
                Some(dtype),
                true,
                ctx.placement_by_slug.get(&slug).cloned(),
            )
        } else {
            (
                child
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("{} #{}", child.kind, child.id)),
                None,
                false,
                None,
            )
        };
        // Objet aperçuable (média/tour/dashboard) : clic → panneau d'aperçu.
        let target = (!is_device).then(|| PreviewTarget {
            kind: child.kind.clone(),
            id: child.id.clone(),
            name: name.clone(),
        });
        rows.push(TreeRow {
            key: format!("{}-{}", child.kind, child.id),
            depth: depth + 1,
            is_folder: false,
            expanded_now: false,
            has_children: false,
            emoji: None,
            icon_kind: Some(child.kind.clone()),
            subtitle,
            is_device,
            is_dead: false,
            is_dragged: ctx
                .dragging
                .as_ref()
                .map(|d| d.kind == child.kind && d.id == child.id)
                .unwrap_or(false),
            toggle: None,
            drag: Some(DragItem {
                kind: child.kind.clone(),
                id: child.id.clone(),
                label: name.clone(),
                edge_id: None,
                from: DropId::Folder(fid.clone()),
            }),
            drop_id: None,
            is_previewed: ctx
                .preview
                .as_ref()
                .zip(target.as_ref())
                .is_some_and(|(p, t)| p.kind == t.kind && p.id == t.id),
            is_pinned: ctx
                .pinned
                .as_ref()
                .zip(target.as_ref())
                .is_some_and(|(p, t)| p.kind == t.kind && p.id == t.id),
            preview: target.clone(),
            pin_target: target.clone(),
            clear_target: target,
            detach: Some(Detach::Contained {
                kind: child.kind.clone(),
                id: child.id.clone(),
            }),
            delete: None,
            labels: None,
            placement,
            name,
        });
    }
}
