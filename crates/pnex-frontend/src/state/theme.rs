//! Colour theme (design-system.md): the profile preference light / dark /
//! auto becomes `data-theme` on `<html>`, which switches the CSS variables
//! of `style/dark.css`. `auto` follows the OS and keeps listening to it.

use crate::storage::{self, KeyValueStorage, KEY_THEME};

/// Normalises the profile value (the backend serialises `Light`/`Dark`/`Auto`).
pub fn normalize(pref: &str) -> &'static str {
    match pref.to_ascii_lowercase().as_str() {
        "light" => "light",
        "dark" => "dark",
        _ => "auto",
    }
}

/// Applies a preference now and remembers it for the next boot.
pub fn apply(pref: &str) {
    let pref = normalize(pref);
    storage::local().set(KEY_THEME, pref);
    let js = format!(
        "(function(p){{var m=window.matchMedia('(prefers-color-scheme: dark)');\
         var f=function(){{document.documentElement.setAttribute('data-theme',\
         p==='dark'||(p==='auto'&&m.matches)?'dark':'light');}};\
         if(window.__pnexTheme){{m.removeEventListener('change',window.__pnexTheme);}}\
         window.__pnexTheme=p==='auto'?f:null;if(p==='auto'){{m.addEventListener('change',f);}}f();}})('{pref}')"
    );
    let _ = dioxus::document::eval(&js);
}

/// Replays the last preference at app start (before the session resolves).
pub fn restore() {
    let pref = storage::local().get(KEY_THEME).unwrap_or_default();
    apply(&pref);
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn normalizes_backend_values() {
        assert_eq!(normalize("Dark"), "dark");
        assert_eq!(normalize("light"), "light");
        assert_eq!(normalize("Auto"), "auto");
        assert_eq!(normalize(""), "auto");
    }
}
