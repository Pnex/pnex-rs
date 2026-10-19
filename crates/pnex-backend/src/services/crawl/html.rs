//! HTML as the server crawler reads it (D307): the title, the readable
//! text (scripts, styles and hidden templates dropped, blocks on their own
//! lines) and the links. Nothing is ever executed.

use ego_tree::NodeRef;
use scraper::{Html, Node, Selector};

/// Elements whose content is never text.
const SKIP: [&str; 6] = ["script", "style", "noscript", "template", "svg", "head"];
/// Elements that start a new line.
const BLOCK: [&str; 22] = [
    "p", "div", "br", "li", "ul", "ol", "tr", "td", "th", "table", "section", "article", "header",
    "footer", "nav", "main", "aside", "h1", "h2", "h3", "h4", "h5",
];

pub struct Page {
    pub title: String,
    pub text: String,
    pub links: Vec<String>,
}

pub fn read(html: &str, base: &reqwest::Url) -> Page {
    let doc = Html::parse_document(html);
    let title = Selector::parse("title")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|t| squash(&t.text().collect::<String>()))
        .unwrap_or_default();
    let mut raw = String::new();
    walk(doc.tree.root(), &mut raw);
    let text = raw
        .lines()
        .map(squash)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let mut links = Vec::new();
    if let Ok(sel) = Selector::parse("a[href]") {
        for a in doc.select(&sel) {
            let Some(href) = a.value().attr("href") else {
                continue;
            };
            if let Ok(mut u) = base.join(href.trim()) {
                u.set_fragment(None);
                links.push(u.to_string());
            }
        }
    }
    Page { title, text, links }
}

fn walk(node: NodeRef<'_, Node>, out: &mut String) {
    match node.value() {
        Node::Text(t) => {
            out.push_str(t);
            out.push(' ');
        }
        Node::Element(e) => {
            let name = e.name();
            if SKIP.contains(&name) {
                return;
            }
            let block = BLOCK.contains(&name) || name == "h6";
            if block {
                out.push('\n');
            }
            for child in node.children() {
                walk(child, out);
            }
            if block {
                out.push('\n');
            }
        }
        _ => {
            for child in node.children() {
                walk(child, out);
            }
        }
    }
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `<loc>` values of a sitemap or sitemap index (XML, no DTD).
pub fn sitemap_locs(xml: &str) -> Vec<String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let (mut out, mut in_loc) = (Vec::new(), false);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => in_loc = e.local_name().as_ref() == b"loc",
            Ok(Event::Text(t)) if in_loc => {
                if let Ok(s) = t.decode() {
                    out.push(s.trim().to_string());
                }
            }
            Ok(Event::End(_)) => in_loc = false,
            Ok(Event::Eof) | Err(_) | Ok(Event::DocType(_)) => break,
            _ => {}
        }
        if out.len() >= 50_000 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_title_and_links_without_scripts() {
        let base = reqwest::Url::parse("https://of.example.fr/formations/").unwrap();
        let p = read(
            r#"<html><head><title> Nos  formations </title><script>var x="RNCP999"</script></head>
            <body><h1>Titre RNCP12345</h1><p>Éligible <b>CPF</b></p><style>.a{}</style>
            <a href="rncp12345#tarifs">Voir</a><a href="/contact">C</a><a href="mailto:x@y.fr">m</a></body></html>"#,
            &base,
        );
        assert_eq!(p.title, "Nos formations");
        assert!(p.text.contains("Titre RNCP12345"));
        assert!(p.text.contains("Éligible CPF"));
        assert!(!p.text.contains("RNCP999"));
        assert_eq!(p.links[0], "https://of.example.fr/formations/rncp12345");
        assert_eq!(p.links[1], "https://of.example.fr/contact");
    }

    #[test]
    fn sitemap_locations() {
        let xml = r#"<?xml version="1.0"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"><url><loc> https://e.fr/a </loc></url><url><loc>https://e.fr/b</loc></url></urlset>"#;
        assert_eq!(sitemap_locs(xml), vec!["https://e.fr/a", "https://e.fr/b"]);
    }
}
