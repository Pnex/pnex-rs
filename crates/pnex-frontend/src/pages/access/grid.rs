//! Permission grid of a role (iam.md §9): the services of the registry in
//! rows, one chip per verb; a click cycles a permission through
//! « not given » → « allowed » → « denied ».

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_api_contract::iam::PERMISSIONS;

/// State of one permission in the role being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    Allow,
    Deny,
}

/// The registry's services (in order) with their verbs.
fn services() -> Vec<(&'static str, Vec<&'static str>)> {
    let mut out: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
    for (p, _) in PERMISSIONS {
        let Some((service, verb)) = p.split_once(':') else {
            continue;
        };
        // A custom role never holds access management (D293).
        if service == "iam" {
            continue;
        }
        match out.last_mut() {
            Some((s, verbs)) if *s == service => verbs.push(verb),
            _ => out.push((service, vec![verb])),
        }
    }
    // Reading first, then writing, then the rest; deleting last.
    let rank = |v: &str| match v {
        "read" => 0,
        "write" => 1,
        "delete" => 9,
        _ => 5,
    };
    for (_, verbs) in out.iter_mut() {
        verbs.sort_by_key(|v| (rank(v), *v));
    }
    out
}

/// Exact permissions of patterns (the grid shows these; wildcards stay as
/// they are, listed apart).
pub(crate) fn split_patterns(patterns: &[String]) -> (Vec<String>, Vec<String>) {
    patterns
        .iter()
        .cloned()
        .partition(|p| PERMISSIONS.iter().any(|(k, _)| k == p))
}

#[component]
pub(crate) fn PermissionGrid(marks: Signal<BTreeMap<String, Mark>>, readonly: bool) -> Element {
    let rows = services();
    rsx! {
        div { class: "max-h-96 overflow-y-auto rounded-md border border-line divide-y divide-line",
            for (service, verbs) in rows {
                div {
                    key: "{service}",
                    class: "flex flex-wrap items-center gap-2 px-3 py-2",
                    span { class: "w-48 shrink-0 text-sm text-ink",
                        {t!(& format!("iam-service-{}", service.replace('.', "-")))}
                    }
                    for verb in verbs {
                        PermissionChip {
                            key: "{service}:{verb}",
                            permission: format!("{service}:{verb}"),
                            verb: verb.to_string(),
                            marks,
                            readonly,
                        }
                    }
                }
            }
        }
    }
}

/// Next state of a chip: not given → allowed → denied → not given.
fn cycle(mut marks: Signal<BTreeMap<String, Mark>>, permission: &str, mark: Option<Mark>) {
    let next = match mark {
        None => Some(Mark::Allow),
        Some(Mark::Allow) => Some(Mark::Deny),
        Some(Mark::Deny) => None,
    };
    let mut m = marks.write();
    match next {
        Some(v) => {
            m.insert(permission.to_string(), v);
        }
        None => {
            m.remove(permission);
        }
    }
}

#[component]
fn PermissionChip(
    permission: String,
    verb: String,
    marks: Signal<BTreeMap<String, Mark>>,
    readonly: bool,
) -> Element {
    let mark = marks.read().get(&permission).copied();
    let (class, state) = match mark {
        None => ("border-line text-slate", t!("access-mark-none")),
        Some(Mark::Allow) => (
            "border-signal-200 bg-signal-50 text-signal-700",
            t!("access-mark-allow"),
        ),
        Some(Mark::Deny) => (
            "border-alarm text-alarm line-through",
            t!("access-mark-deny"),
        ),
    };
    let label = t!(&format!("iam-verb-{verb}"));
    let aria = format!("{label} — {state}");
    rsx! {
        button {
            r#type: "button",
            class: "rounded-full border px-2.5 py-0.5 text-xs {class} disabled:cursor-default",
            disabled: readonly,
            title: "{permission}",
            aria_label: "{aria}",
            onclick: move |_| cycle(marks, &permission, mark),
            {label}
        }
    }
}
