use super::helpers::*;
use super::*;

use crate::app::Route;
use crate::components::ui::FIELD_LABEL;
use pnex_core::site_recipe::SiteRecipeNodeConfig;

// ─────────────── site recipe (site-recipes.md D327) ───────────────

/// Website rule inspector: the saved rule played at each message and the
/// values of its parameters.
#[component]
pub(super) fn SiteRecipeForm(
    mut cx: EditorCx,
    initial: SiteRecipeNodeConfig,
    can_write: bool,
) -> Element {
    let recipes_res = use_resource(|| async { api::site_recipes::all().await.unwrap_or_default() });
    let mut config = use_signal(move || initial.clone());
    let recipes = recipes_res.value().read().clone().unwrap_or_default();
    let loaded = recipes_res.value().read().is_some();
    let current = config();
    let picked = recipes.iter().find(|r| r.id == current.recipe_id).cloned();
    let params: Vec<String> = picked
        .as_ref()
        .and_then(|r| r.recipe.as_ref())
        .map(|r| r.params.clone())
        .unwrap_or_default();

    let mut set = move |next: SiteRecipeNodeConfig| {
        config.set(next.clone());
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::SiteRecipe { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let on_pick = move |event: Event<FormData>| {
        set(SiteRecipeNodeConfig {
            recipe_id: event.value(),
            params: Default::default(),
        });
    };

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-site-recipe-help")} }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-site-recipe-rule")} }
                if loaded && recipes.is_empty() {
                    p { class: "text-xs text-amber-700", {t!("flows-site-recipe-none")} }
                }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: on_pick,
                    option { value: "", selected: current.recipe_id.is_empty(), "—" }
                    for r in recipes.iter() {
                        option {
                            key: "{r.id}",
                            value: "{r.id}",
                            selected: r.id == current.recipe_id,
                            "{r.site} · {r.name}"
                        }
                    }
                }
                if loaded && !current.recipe_id.is_empty() && picked.is_none() {
                    p { class: "text-xs text-red-700 mt-1", {t!("flows-site-recipe-unknown")} }
                }
            }
            for name in params {
                label { key: "{name}", class: "block",
                    span { class: FIELD_LABEL, "{name}" }
                    input {
                        class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                        disabled: !can_write,
                        value: current.params.get(&name).cloned().unwrap_or_default(),
                        oninput: {
                            let name = name.clone();
                            move |event: Event<FormData>| {
                                let mut next = config.peek().clone();
                                next.params.insert(name.clone(), event.value());
                                set(next);
                            }
                        },
                    }
                }
            }
            if let Some(r) = picked {
                Link {
                    class: "text-xs text-signal-700 hover:underline",
                    to: Route::WebSiteStudio {
                        site: r.site.clone(),
                    },
                    new_tab: true,
                    {t!("flows-site-recipe-open-studio")}
                }
            }
        }
    }
}
