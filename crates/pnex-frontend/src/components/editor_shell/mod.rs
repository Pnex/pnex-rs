//! Unified editor shell — the chrome shared by the three editors
//! (flows, dashboards, studio): 3-zone top bar, banner,
//! full-screen body (canvas + floating palette + disclosing inspector),
//! empty-state invitation. The editors keep owning their state
//! (`EditorCx` + reducers) and their canvas; they pass `Element` slots
//! here (same approach as `ListLayout`: "proven" props, chrome i18n
//! resolved internally, the content comes from the caller).
//!
//! Principles (mock-up validated 2026-09):
//! 1. single bar — left: back + name + status · right: version + actions;
//! 2. uniform status — `StatusChip` dot + label, neutral version chip `v{n}`;
//! 3. palette on demand — floating `+` button → `PalettePopover`;
//! 4. inspector only on selection — `InspectorPanel`, absent otherwise;
//! 5. empty state = invitation — "Click + to get started".

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;

pub mod floating_panel;
pub mod inspector;
pub mod palette;

pub use inspector::InspectorPanel;
pub use palette::{PaletteIcon, PaletteItem, PalettePopover};

/// Tone of a chip — pill + dot, full literal classes
/// (Tailwind scan requirement: never a built class).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusTone {
    Green,
    Amber,
    Red,
    Slate,
    Purple,
}

impl StatusTone {
    /// Pill classes (border/background/text), tone by tone.
    fn pill(self) -> &'static str {
        match self {
            StatusTone::Green => "border-green-200 bg-green-50 text-green-700",
            StatusTone::Amber => "border-amber-200 bg-amber-50 text-amber-700",
            StatusTone::Red => "border-red-200 bg-red-50 text-red-700",
            StatusTone::Slate => "border-gray-200 bg-gray-50 text-gray-600",
            StatusTone::Purple => "border-purple-200 bg-purple-50 text-purple-700",
        }
    }

    /// Classes of the coloured dot.
    fn dot(self) -> &'static str {
        match self {
            StatusTone::Green => "bg-green-500",
            StatusTone::Amber => "bg-amber-500",
            StatusTone::Red => "bg-red-500",
            StatusTone::Slate => "bg-gray-400",
            StatusTone::Purple => "bg-purple-500",
        }
    }
}

/// Status shown in the bar — the same language for the three editors:
/// a coloured dot + a label (+ optional error tooltip).
#[derive(Clone, PartialEq, Debug)]
pub struct EditorStatus {
    pub tone: StatusTone,
    pub label: String,
    pub tooltip: Option<String>,
}

impl EditorStatus {
    pub fn new(tone: StatusTone, label: impl Into<String>) -> Self {
        Self {
            tone,
            label: label.into(),
            tooltip: None,
        }
    }

    pub fn with_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }
}

/// Status pill — coloured dot + label (principle 2).
#[component]
pub fn StatusChip(status: EditorStatus) -> Element {
    let pill = format!(
        "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium {}",
        status.tone.pill()
    );
    let dot = format!("h-2 w-2 rounded-full {}", status.tone.dot());
    rsx! {
        span { class: "{pill}", title: status.tooltip.unwrap_or_default(),
            span { class: "{dot}" }
            {status.label}
        }
    }
}

/// Side pill (unsaved changes, old version loaded…)
/// — same visual language as `StatusChip`, `on_remove` adds the cross.
#[component]
pub fn Chip(
    tone: StatusTone,
    label: String,
    #[props(default)] on_remove: Option<Callback<()>>,
) -> Element {
    let pill = format!(
        "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium {}",
        tone.pill()
    );
    let dot = format!("h-1.5 w-1.5 rounded-full {}", tone.dot());
    rsx! {
        span { class: "{pill}",
            span { class: "{dot}" }
            {label}
            if let Some(cb) = on_remove {
                button {
                    class: "ml-0.5 opacity-70 hover:opacity-100",
                    onclick: move |_| cb.call(()),
                    icons::X { class: "h-3 w-3" }
                }
            }
        }
    }
}

/// The shell — see the module docs for the contract.
#[component]
pub fn EditorShell(
    /// Back to the parent list.
    on_back: Callback<()>,
    /// Name of the edited item.
    title: String,
    /// Side reference ("#12") — None until the detail is loaded.
    #[props(default)]
    subtitle: Option<String>,
    /// Current status (dot + label).
    status: EditorStatus,
    /// Current version — neutral "v{n}" chip (rendered here, zero i18n).
    #[props(default)]
    version: Option<i64>,
    /// Side chips (dirty, old version loaded…).
    #[props(default)]
    extra_chips: Option<Element>,
    /// Right zone: the editor's action buttons (save, deploy…).
    actions: Element,
    /// Banner under the bar (violations, "Deploy?" notice, read-only).
    #[props(default)]
    banner: Option<Element>,
    /// Canvas — rendered in `div.absolute.inset-0`: the root of the canvas
    /// component must size itself on its parent (`h-full w-full`).
    canvas: Element,
    /// Floating `+` button + palette popover (`PalettePopover`).
    #[props(default)]
    palette: Option<Element>,
    /// Floating tools pill next to the `+` (mode tools, undo/redo…).
    #[props(default)]
    tools: Option<Element>,
    /// A list column (`MapPanel`) is open at the canvas's left edge: the
    /// `+` and the tools float right of it (desktop; on phones it overlays).
    #[props(default)]
    palette_inset: bool,
    /// Inspector (`InspectorPanel`) — None ⇒ absent from the screen (principle 4).
    #[props(default)]
    inspector: Option<Element>,
    /// Empty-state invitation — Some ⇒ centred veil "Click + …".
    #[props(default)]
    empty_hint: Option<String>,
    /// Inline rename — Some ⇒ the title becomes editable on click
    /// (Enter / blur confirm, Escape cancels). Reserved for authorised
    /// users (the editor only passes it if `can_write`).
    #[props(default)]
    on_rename: Option<Callback<String>>,
) -> Element {
    let mut renaming = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let rename_title = t!("eshell-rename");
    // Two closures (`onkeydown`/`onblur`) each consume their own copy
    // (a String cannot be moved into two `move` closures).
    // Hide the mobile app header while this editor is mounted (see
    // `ui::EDITORS_OPEN`); the effect reads no signal, so it runs once.
    use_effect(|| {
        *crate::state::ui::EDITORS_OPEN.write() += 1;
    });
    use_drop(|| {
        let open = *crate::state::ui::EDITORS_OPEN.peek();
        *crate::state::ui::EDITORS_OPEN.write() = open.saturating_sub(1);
    });
    // Menus close on a pointer press outside any floating menu or on
    // Escape (capture phase: canvas handlers stopping propagation still
    // count). The listener lives while the shell is mounted.
    use_effect(|| {
        spawn(async move {
            let mut ev = document::eval(
                r#"
                const prev = window.__pnexMenuDismiss;
                if (prev) {
                    document.removeEventListener('pointerdown', prev.down, true);
                    document.removeEventListener('keydown', prev.key, true);
                }
                const down = (e) => {
                    const t = e.target;
                    if (!(t && t.closest && t.closest('[data-floating]'))) dioxus.send(true);
                };
                const key = (e) => { if (e.key === 'Escape') dioxus.send(true); };
                document.addEventListener('pointerdown', down, true);
                document.addEventListener('keydown', key, true);
                window.__pnexMenuDismiss = { down, key };
                await new Promise(() => {});
                "#,
            );
            while ev.recv::<bool>().await.is_ok() {
                crate::state::ui::close_menus();
            }
        });
    });
    use_drop(|| {
        crate::state::ui::close_menus();
        let _ = document::eval(
            r#"
            const prev = window.__pnexMenuDismiss;
            if (prev) {
                document.removeEventListener('pointerdown', prev.down, true);
                document.removeEventListener('keydown', prev.key, true);
                window.__pnexMenuDismiss = null;
            }
            "#,
        );
    });
    let title_enter = title.clone();
    let title_blur = title.clone();
    rsx! {
        // Full height: the mobile app header is hidden while an editor is
        // open (`ui::EDITORS_OPEN`); desktop content column already carries
        // the sidebar offset (lg:pl-64/16).
        div { class: "flex h-[100dvh] flex-col lg:h-screen",
            // ─── Zone 1/2: top bar (left identity · right version+actions)
            // Phone: two rows (identity, then actions right-aligned) instead
            // of one 680 px row scrolled sideways; single 56 px row from sm up.
            div { class: "flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1.5 bg-white border-b border-gray-200 px-3 py-2 sm:h-14 sm:flex-nowrap sm:overflow-x-auto sm:py-0",
                button {
                    class: "inline-flex shrink-0 items-center gap-1.5 rounded-lg px-2 py-1.5 text-sm text-gray-600 hover:bg-gray-100",
                    onclick: move |_| on_back.call(()),
                    icons::ArrowLeft { class: "h-4 w-4" }
                    span { class: "hidden sm:inline", {t!("eshell-back")} }
                }
                div { class: "h-5 w-px shrink-0 bg-gray-200" }
                // Title: plain when there is no rename; otherwise click → inline
                // input (Enter/blur confirm, Escape cancels, empty ignored).
                {
                    match on_rename {
                        Some(cb) if renaming() => rsx! {
                            input {
                                class: "w-56 shrink-0 rounded-lg border border-signal-400 px-2 py-1 text-sm font-semibold text-gray-900 focus:outline-none focus:ring-2 focus:ring-signal-500",
                                value: "{draft}",
                                autofocus: true,
                                oninput: move |e: Event<FormData>| draft.set(e.value()),
                                onkeydown: move |e: KeyboardEvent| {
                                    if e.key() == Key::Escape {
                                        renaming.set(false);
                                    } else if e.key() == Key::Enter {
                                        renaming.set(false);
                                        let value = draft().trim().to_string();
                                        if !value.is_empty() && value != title_enter {
                                            cb.call(value);
                                        }
                                    }
                                },
                                onblur: move |_| {
                                    renaming.set(false);
                                    let value = draft().trim().to_string();
                                    if !value.is_empty() && value != title_blur {
                                        cb.call(value);
                                    }
                                },
                            }
                        },
                        Some(_) => rsx! {
                            button {
                                class: "group -ml-1 flex min-w-0 items-center gap-1 rounded-lg px-1.5 py-0.5 hover:bg-gray-100 sm:shrink-0",
                                title: "{rename_title}",
                                onclick: move |_| {
                                    draft.set(title.clone());
                                    renaming.set(true);
                                },
                                span { class: "min-w-0 max-w-[15rem] truncate text-base font-semibold text-gray-900",
                                    {title.clone()}
                                }
                                icons::Pencil { class: "h-3.5 w-3.5 shrink-0 text-gray-500 group-hover:text-gray-600" }
                            }
                        },
                        None => rsx! {
                            h2 { class: "min-w-0 max-w-[16rem] truncate text-base font-semibold text-gray-900 sm:shrink-0",
                                {title}
                            }
                        },
                    }
                }
                if let Some(sub) = subtitle {
                    span { class: "hidden shrink-0 text-xs text-gray-500 sm:inline",
                        {sub}
                    }
                }
                StatusChip { status }
                // Side chips (dirty, old version…)
                {extra_chips}
                div { class: "hidden min-w-4 flex-1 sm:block" }
                if let Some(v) = version {
                    span { class: "hidden shrink-0 items-center rounded-full border border-gray-200 bg-gray-50 px-2.5 py-0.5 text-xs font-medium text-gray-600 sm:inline-flex",
                        {format!("v{v}")}
                    }
                }
                // Zone 3: editor actions (existing buttons, moved).
                div { class: "flex w-full items-center justify-end gap-2 overflow-x-auto sm:w-auto sm:shrink-0 sm:overflow-visible",
                    {actions}
                }
            }
            // ─── Banner (violations / read-only / deploy notice)
            if let Some(b) = banner {
                div { class: "shrink-0 space-y-2 px-3 pt-2", {b} }
            }
            // ─── Body: full-screen canvas + on-demand overlays
            div { class: "relative min-h-0 flex-1",
                div { class: "absolute inset-0", {canvas} }
                if let Some(hint) = empty_hint {
                    div { class: "pointer-events-none absolute inset-0 z-10 flex items-center justify-center",
                        p { class: "rounded-lg border border-line bg-surface/90 px-4 py-2 text-sm text-slate",
                            {hint}
                        }
                    }
                }
                // `data-floating`: a press inside a menu (or on its toggle
                // button) never counts as a click outside.
                if let Some(p) = palette {
                    div {
                        class: if palette_inset { "absolute left-4 top-4 z-20 lg:left-[21rem]" } else { "absolute left-4 top-4 z-20" },
                        "data-floating": "true",
                        {p}
                    }
                }
                if let Some(t) = tools {
                    div {
                        class: if palette_inset { "absolute left-[4.5rem] top-4 z-20 lg:left-[24.5rem]" } else { "absolute left-[4.5rem] top-4 z-20" },
                        "data-floating": "true",
                        {t}
                    }
                }
                {inspector}
            }
        }
    }
}
