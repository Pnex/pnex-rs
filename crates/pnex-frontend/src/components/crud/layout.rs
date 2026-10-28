//! Canonical layout of "list of a resource" pages — the shell of the
//! CRUD pattern: header (title, subtitle, back button, "add" action
//! gated by `can_write`), filters slot, body. No visual decision here:
//! the classes are the historical literals of the pages (devices/flows),
//! kept identical — including for the button consts shared
//! with `form.rs`.
//!
//! What deliberately stays **in the page**: the org guard
//! (`org::current().is_none()` → "orgs-empty" message), the `can_write`
//! computation (private helper per page — project convention) and i18n
//! (`t!` on the caller side, the base only knows `String`s).

use dioxus::prelude::*;

use crate::components::icons;
use crate::components::ui::BTN_SECONDARY;

/// Primary button — header "add" action, form submits.
pub const PRIMARY_BTN: &str = crate::components::ui::BTN_PRIMARY;

/// Cancel in form footers: the secondary button of the design system.
pub const GHOST_BTN: &str = crate::components::ui::BTN_SECONDARY;

/// Danger action — row-level "delete" of CRUD list tables (canonical
/// style: bordered red + trash icon, icon rendered by the caller since
/// the label stays i18n-resolved by the page).
pub const DANGER_BTN: &str = crate::components::ui::BTN_DANGER;

#[component]
pub fn ListLayout(
    /// Page title (i18n resolved by the caller).
    title: String,
    /// Optional subtitle (i18n resolved by the caller).
    #[props(default)]
    subtitle: Option<String>,
    /// Back to the parent list — bordered button with an enlarged icon
    /// (`h-5 w-5`, readable next to the `text-3xl` title; the historical
    /// `h-4 w-4` was too discreet).
    #[props(default)]
    on_back: Option<Callback<()>>,
    /// Label of the add button (i18n resolved by the caller).
    #[props(default)]
    add_label: Option<String>,
    /// "Add" click — primary button rendered iff `can_write` and
    /// `on_add` + `add_label` are provided.
    #[props(default)]
    on_add: Option<Callback<()>>,
    /// Slot for extra header actions (multi-button pages:
    /// media = camera capture + take360 before upload). Rendered on the
    /// right, before the "add" button.
    #[props(default)]
    actions: Option<Element>,
    /// Manual refresh — THE single place of the refresh button on every
    /// list page: last button of the header, right after the add button.
    /// Never in a search bar, a tab row or a filter row.
    #[props(default)]
    on_refresh: Option<Callback<()>>,
    /// Render the header row at all — full-page detail subviews that draw
    /// their own header (functions editor) pass `false` to drop the CRUD
    /// title / add-button row entirely.
    #[props(default = true)]
    header: bool,
    /// Owner/admin write right — computed by the page (the server
    /// enforces, the UI hides).
    can_write: bool,
    /// Filters slot under the header: pass a `FilterBar` (same rendering
    /// as a `FilterBar` placed in the page body).
    #[props(default)]
    filters: Option<Element>,
    /// Tab bar of a page split in tabs, right under the header.
    #[props(default)]
    tabs: Option<Element>,
    /// Beside the title (a step line, a status): stays on the title row.
    #[props(default)]
    title_aside: Option<Element>,
    children: Element,
) -> Element {
    rsx! {
        // Phone: tighter gutters, bottom room so the floating assistant
        // button never covers the last row; desktop values from md up.
        div { class: "p-4 pb-24 md:p-6",
            if header {
                div { class: "mb-4 md:mb-8 flex items-center justify-between flex-wrap gap-3",
                    // The title block takes the free width and wraps a long
                    // subtitle, so the buttons stay on the right.
                    div { class: "flex min-w-0 flex-1 basis-72 flex-wrap items-center gap-3",
                        if let Some(back) = on_back {
                            button {
                                class: BTN_SECONDARY,
                                r#type: "button",
                                onclick: move |_| back.call(()),
                                icons::ArrowLeft { class: "h-5 w-5" }
                            }
                        }
                        TitleBlock { title, subtitle }
                        if let Some(aside) = title_aside {
                            div { class: "md:ml-4", {aside} }
                        }
                    }
                    // One right-aligned group: extra actions sit next to
                    // the add button, never floating mid-header.
                    div { class: "ml-auto flex shrink-0 items-center gap-2 flex-wrap",
                        if let Some(actions) = actions {
                            {actions}
                        }
                        if can_write {
                            // Documented contract: "add" button rendered iff
                            // `on_add` AND `add_label` are provided (add_label None =
                            // page in detail mode, no creation here).
                            if let (Some(add), Some(label)) = (on_add, add_label) {
                                button {
                                    class: PRIMARY_BTN,
                                    r#type: "button",
                                    "data-testid": "list-add",
                                    onclick: move |_| add.call(()),
                                    icons::Plus { class: "h-4 w-4 inline mr-1" }
                                    {label}
                                }
                            }
                        }
                        if let Some(refresh) = on_refresh {
                            crate::components::crud::filters::RefreshButton { on_click: refresh }
                        }
                    }
                }
            }
            if let Some(tabs) = tabs {
                div { class: "mb-4 md:mb-6", {tabs} }
            }
            // The slot takes a `FilterBar`, which carries the layout: no
            // second wrapper (doubled margin, unstretched inputs).
            if let Some(filters) = filters {
                {filters}
            }
            {children}
        }
    }
}

/// Header title + subtitle block.
#[component]
fn TitleBlock(title: String, subtitle: Option<String>) -> Element {
    rsx! {
        div {
            h1 { class: "text-2xl md:text-3xl font-bold text-gray-900", {title} }
            if let Some(sub) = subtitle {
                p { class: "text-sm md:text-base text-gray-600 mt-1 md:mt-2", {sub} }
            }
        }
    }
}
