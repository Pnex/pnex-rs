//! Layout racine — porté du `Layout.tsx` React (sidebar grise-900 fixe en
//! desktop, drawer mobile) + garde de session : rend `Login` à la place de
//! l'`Outlet` tant que l'utilisateur n'est pas authentifié (parité
//! `AuthWrapper` React — pas de route `/login`, une seule URL canonique).

use crate::app::Route;
use dioxus::prelude::*;

#[component]
pub fn Shell() -> Element {
    rsx! { Outlet::<Route> {} }
}
