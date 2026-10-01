//! Vision model names cache (D81): model id → name, for the flow canvas
//! subtitle of `vision-detect` nodes. Filled by the flow editor (one fetch
//! per editor mount), the vision inspector and the Models page.

use std::collections::HashMap;

use dioxus::prelude::*;

pub static MODEL_NAMES: GlobalSignal<HashMap<String, String>> = GlobalSignal::new(HashMap::new);

/// Name of a registered model, if known.
pub fn model_name(id: &str) -> Option<String> {
    MODEL_NAMES.read().get(id).cloned()
}

/// Replaces the cache with a fresh model list.
pub fn remember(models: &[pnex_core::vision::MlModel]) {
    let map: HashMap<String, String> = models
        .iter()
        .map(|m| (m.id.clone(), m.name.clone()))
        .collect();
    MODEL_NAMES.with_mut(|names| *names = map);
}
