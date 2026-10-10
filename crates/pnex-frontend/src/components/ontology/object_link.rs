//! "Object page" link of the specialised pages (D186): devices, media,
//! dashboards… stay their own pages and each gains a link to the generic
//! object page of their identity.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::app::Route;
use crate::components::icons;

/// Resolves `kind/native_id` to its object and links to its page; renders
/// nothing while unresolved (identity missing or no access).
#[component]
pub fn ObjectPageLink(kind: String, native_id: String) -> Element {
    let res = use_resource(move || {
        let (kind, native_id) = (kind.clone(), native_id.clone());
        async move { api::ontology::resolve(&kind, &native_id).await.ok() }
    });
    let Some(Some(object)) = res.read().clone() else {
        return rsx! {};
    };
    rsx! {
        Link {
            class: "inline-flex items-center gap-1 px-3 py-1 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50",
            to: Route::OntologyObject {
                id: object.id,
            },
            "data-testid": "object-page-link",
            icons::Shapes { class: "h-4 w-4" }
            {t!("onto-open-object")}
        }
    }
}
