//! Design-system primitives (docs/architecture/design-system.md): the one
//! place where a button, a field or a state looks like something. Pages
//! compose these classes and components instead of spelling Tailwind
//! utilities by hand.

// Primitives land before every consumer migrates (same as icons.rs).
#![allow(dead_code)]

use dioxus::prelude::*;

/// Main action of a screen: ink fill, one per screen.
pub const BTN_PRIMARY: &str = "inline-flex items-center justify-center gap-1.5 min-h-10 px-4 py-2 rounded-md bg-ink text-white text-sm font-medium hover:bg-gray-800 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-signal disabled:opacity-40 disabled:cursor-not-allowed active:scale-[0.97] motion-reduce:active:scale-100 transition-[color,background-color,border-color,scale] duration-150 ease-out-ui";

/// Secondary action (cancel, export…): bordered surface.
pub const BTN_SECONDARY: &str = "inline-flex items-center justify-center gap-1.5 min-h-10 px-4 py-2 rounded-md border border-line-strong bg-surface text-ink text-sm font-medium hover:bg-gray-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-signal disabled:opacity-40 disabled:cursor-not-allowed active:scale-[0.97] motion-reduce:active:scale-100 transition-[color,background-color,border-color,scale] duration-150 ease-out-ui";

/// Compact primary for toolbars and table rows.
pub const BTN_PRIMARY_SM: &str = "inline-flex items-center justify-center gap-1.5 px-3 py-1.5 rounded-md bg-ink text-white text-sm font-medium hover:bg-gray-800 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-signal disabled:opacity-40 disabled:cursor-not-allowed transition-colors";

/// Compact secondary for toolbars and table rows.
pub const BTN_SECONDARY_SM: &str = "inline-flex items-center justify-center gap-1.5 px-3 py-1.5 rounded-md border border-line-strong bg-surface text-ink text-sm font-medium hover:bg-gray-50 focus-visible:outline-2 focus-visible:outline-signal disabled:opacity-40 disabled:cursor-not-allowed transition-colors";

/// Low-emphasis action inside a row or a toolbar.
pub const BTN_GHOST: &str = "inline-flex items-center justify-center gap-1.5 px-3 py-1.5 rounded-md text-sm font-medium text-signal hover:bg-signal-soft focus-visible:outline-2 focus-visible:outline-signal disabled:opacity-40 transition-colors";

/// Destructive action before confirmation: alarm text on a surface.
pub const BTN_DANGER: &str = "inline-flex items-center justify-center gap-1.5 px-3 py-1.5 rounded-md border border-red-200 bg-surface text-alarm text-sm font-medium hover:bg-red-50 focus-visible:outline-2 focus-visible:outline-alarm disabled:opacity-40 transition-colors";

/// Confirmed destruction only (inside the confirmation dialog).
pub const BTN_DANGER_SOLID: &str = "inline-flex items-center justify-center gap-1.5 min-h-10 px-4 py-2 rounded-md bg-alarm text-white text-sm font-medium hover:bg-red-800 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-alarm disabled:opacity-40 active:scale-[0.97] motion-reduce:active:scale-100 transition-[color,background-color,border-color,scale] duration-150 ease-out-ui";

/// Text input, select and textarea.
pub const INPUT: &str = "w-full rounded-md border border-line-strong bg-surface px-3 py-2 text-sm text-ink placeholder:text-gray-500 focus:border-signal focus:outline-none focus:ring-3 focus:ring-signal-200 disabled:bg-gray-50 disabled:text-gray-500";

/// Field label, sentence case (never uppercase).
pub const FIELD_LABEL: &str = "block text-sm font-medium text-ink mb-1.5";

/// Help text under a field.
pub const FIELD_HELP: &str = "mt-1.5 text-xs text-slate";

/// Error text under a field.
pub const FIELD_ERROR: &str = "mt-1.5 text-xs text-alarm";

/// Flat work surface (list, form section, detail block).
/// Tab bar (`role: "tablist"`): underlined tabs, the signal line marks the
/// open one. Tabs are views of one page, never actions (no filled pill).
pub const TAB_BAR: &str = "flex flex-wrap gap-x-1 border-b border-line";

/// Closed tab (`role: "tab"`, `aria_selected: false`).
pub const TAB: &str = "-mb-px inline-flex items-center gap-1.5 border-b-2 border-transparent px-3 py-2 text-sm font-medium text-slate hover:text-ink hover:border-line-strong focus-visible:outline-2 focus-visible:outline-signal";

/// Open tab (`role: "tab"`, `aria_selected: true`).
pub const TAB_ACTIVE: &str = "-mb-px inline-flex items-center gap-1.5 border-b-2 border-signal px-3 py-2 text-sm font-semibold text-ink focus-visible:outline-2 focus-visible:outline-signal";

pub const PANEL: &str = "rounded-lg border border-line bg-surface";

/// A D42 label as an asset tag: name on the left, value on the right; a
/// bare tag is the name alone.
#[component]
pub fn LabelTag(name: String, value: Option<String>, children: Element) -> Element {
    rsx! {
        span { class: "inline-flex items-stretch overflow-hidden rounded border border-line-strong bg-surface text-xs leading-4",
            if let Some(value) = value {
                span { class: "bg-gray-50 px-1.5 py-0.5 text-slate border-r border-line-strong",
                    "{name}"
                }
                span { class: "px-1.5 py-0.5 text-ink", "{value}" }
            } else {
                span { class: "px-1.5 py-0.5 text-ink", "{name}" }
            }
            {children}
        }
    }
}

/// Operational state of a thing (ISA-101): colour doubled by a shape so it
/// reads without colour vision.
#[derive(Clone, Copy, PartialEq)]
pub enum State {
    Ok,
    Idle,
    Never,
    Warn,
    Alarm,
}

#[component]
pub fn StateMark(state: State, label: String) -> Element {
    let (text, mark) = match state {
        State::Ok => ("text-ink", "h-2 w-2 rounded-full bg-ok"),
        State::Idle => ("text-slate", "h-2 w-2 rounded-full bg-idle"),
        State::Never => (
            "text-slate",
            "h-2 w-2 rounded-full border-[1.5px] border-idle",
        ),
        State::Warn => ("text-warn-text font-medium", "h-2 w-2 rotate-45 bg-warn"),
        State::Alarm => (
            "text-alarm font-semibold",
            "h-0 w-0 border-x-[5px] border-b-[9px] border-x-transparent border-b-alarm",
        ),
    };
    rsx! {
        span { class: "inline-flex items-center gap-2 text-sm whitespace-nowrap {text}",
            span { class: "shrink-0 {mark}", aria_hidden: "true" }
            "{label}"
        }
    }
}
