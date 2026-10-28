//! Pure model layer of the POI tree: drag payloads, containment tree
//! snapshot and the row-view builder (everything resolved before the rsx).

use super::*;

/// Drop target: POI root or a folder.
#[derive(Clone, PartialEq)]
pub(super) enum DropId {
    Root,
    Folder(String),
}

/// Object grabbed by the handle — everything needed to mutate on drop.
#[derive(Clone)]
pub(super) struct DragItem {
    pub(super) kind: String,
    pub(super) id: String,
    /// Current display name (recreates the edge label on return to root).
    pub(super) label: String,
    /// `placed_on` edge of a root object (deleted if filed in a folder).
    pub(super) edge_id: Option<i64>,
    /// Current location — an identical drop is a no-op.
    pub(super) from: DropId,
}

/// Payload of the Detach button.
#[derive(Clone)]
pub(super) enum Detach {
    /// Root object: the `placed_on` edge goes.
    Edge(i64),
    /// Filed object: ejected to the POI root (containment detached +
    /// `placed_on` edge recreated — a pure detach left the object
    /// orphaned, invisible from every tree, observed 2026-09-13). A filed
    /// device is only detached from the folder (placement intact, D43 amendment).
    Contained { kind: String, id: String },
    /// Root device: the placement goes — the device becomes free again
    /// (D43 amendment, 2026-09-13).
    Placement(i64),
}

/// POI tree state (containment walk, names hydrated by the server).
#[derive(Clone, Default, PartialEq)]
pub(super) struct TreeModel {
    /// Child folders of the POI (server order).
    pub(super) root_folders: Vec<RefNode>,
    /// folder id → children (server order).
    pub(super) children: HashMap<String, Vec<RefNode>>,
    /// Everything that lives under ≥ 1 folder (excluded from the render roots).
    pub(super) contained: HashSet<(String, String)>,
}

// ─────────────────────────── helpers graphe ───────────────────────────

/// Descendants (all kinds) of a folder — anti-cycle guard of the drag.
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
    // Server order kept (sorted by sort_key then kind/id).
    for folder in &model.root_folders {
        push_folder(&ctx, folder, 0, DropId::Root, &mut rows);
    }

    // Root object = unfiled `placed_on` edge (D26 dead links included,
    // always root).
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
        // Previewable object (non-dead media/tour/dashboard): click → panel.
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

    // Root device = unfiled D43 placement — resolved via the local
    // slug→(pk, type) index built from the drawer's device list.
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

/// Push a folder then its children (recursive, expanded branches).
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
    // Folders first (presentation), server order within each group.
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
        // Previewable object (media/tour/dashboard): click → preview panel.
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
