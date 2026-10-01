//! Rendu markdown des réponses de l'assistant IA (components/assistant.rs).
//!
//! L'IA répond en markdown (tables, blocs de code, listes) ; le drawer
//! affichait le texte brut. On détecte le markdown par heuristique, on le
//! convertit en HTML avec pulldown-cmark (tables + barré), puis on injecte
//! via `dangerous_inner_html` — stylé par `.ai-md` (style/tailwind.css).
//!
//! Sanitisation XSS : pulldown-cmark ne filtre PAS le HTML brut, on jette
//! donc tous les événements `Html`/`InlineHtml` du parseur avant rendu.
//! Le texte ordinaire est échappé par le renderer.

use pulldown_cmark::{Event, Options, Parser};

/// Heuristique de détection : la réponse contient-elle du markdown qu'on
/// veut rendre (table, code, gras/italique, titre, liste) ? Sinon on
/// affiche le texte brut comme avant — évite de transformer « 1. » au
/// milieu d'une phrase ou les `#` d'un ID en structure fausse.
pub fn looks_like_markdown(text: &str) -> bool {
    if text.contains("```") {
        return true;
    }
    for line in text.lines() {
        let trimmed = line.trim_start();
        // Table : ligne de séparation |---|---| ou cellules | a | b |
        if trimmed.starts_with('|') && trimmed.matches('|').count() >= 2 {
            return true;
        }
        if trimmed.starts_with('#') {
            return true;
        }
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            return true;
        }
        // Liste ordonnée en début de ligne (pas « 1.5 » au milieu d'une phrase).
        if trimmed.starts_with(|c: char| c.is_ascii_digit())
            && trimmed.contains(". ")
            && trimmed.find(". ") == Some(1)
        {
            return true;
        }
        if text.contains("**") || text.contains("__") {
            return true;
        }
        // Code inline `x` (paire de backticks sur la même ligne).
        if text.matches('`').count() >= 2 {
            return true;
        }
    }
    false
}

/// Convertit le markdown en HTML. Le HTML brut de la source (IA, donc non
/// fiable) est supprimé : seuls les éléments markdown sont rendus.
pub fn to_html(text: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(text, options).filter_map(|event| match event {
        // HTML brut jeté (sanitisation) ; le contenu texte d'un bloc de
        // code arrive en Event::Text, il n'est pas touché.
        Event::Html(_) | Event::InlineHtml(_) => None,
        other => Some(other),
    });
    let mut html = String::with_capacity(text.len() + 64);
    pulldown_cmark::html::push_html(&mut html, parser);
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_covers_common_shapes() {
        assert!(looks_like_markdown("| a | b |\n|---|---|\n| 1 | 2 |"));
        assert!(looks_like_markdown("voici :\n- un\n- deux"));
        assert!(looks_like_markdown("# Titre"));
        assert!(looks_like_markdown("du **gras** ici"));
        assert!(looks_like_markdown("du `code` inline"));
        assert!(looks_like_markdown("```rust\nfn main() {}\n```"));
    }

    #[test]
    fn plain_text_is_not_detected() {
        assert!(!looks_like_markdown("Bonjour, voici la liste des devices."));
        assert!(!looks_like_markdown("Il y a 1.5 mètre de câble."));
        assert!(!looks_like_markdown("device #2 est actif"));
        assert!(!looks_like_markdown(""));
    }

    #[test]
    fn html_raw_is_stripped() {
        // Les balises sont jetées ; le texte entre balises reste affiché,
        // échappé, en texte ordinaire (inerte).
        let html = to_html("texte <script>alert(1)</script> suite");
        assert!(!html.contains("<script"));
        assert!(html.contains("suite"));
        let html = to_html("<img src=x onerror=alert(1)> ok");
        assert!(!html.contains("<img"));
        assert!(html.contains("ok"));
    }

    #[test]
    fn table_and_code_render() {
        let html = to_html("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(html.contains("<table>"));
        assert!(html.contains("<td>1</td>"));
        let html = to_html("```text\nligne 1\nligne 2\n```");
        assert!(html.contains("<pre>"));
        assert!(html.contains("ligne 1"));
    }
}
