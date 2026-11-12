//! `{{name}}` templates of a step (pipeline-engine.md D936): filled from the
//! columns of a row, or `{{setting.<pack>.<key>}}` from the org's settings.
//! A name may end with filters: `{{pair|lower}}`, `{{code|upper}}`,
//! `{{month_m|int}}`, `{{code|before:.}}`, `{{code|after:.}}`.

/// The value of `name|filter|…`; `None` for an unknown name or filter.
fn value(expr: &str, lookup: &impl Fn(&str) -> Option<String>) -> Option<String> {
    let mut parts = expr.split('|').map(str::trim);
    let mut v = lookup(parts.next()?)?;
    for f in parts {
        v = match f {
            "lower" => v.to_lowercase(),
            "upper" => v.to_uppercase(),
            // A number without its leading zeros (`03` → `3`).
            "int" => v.trim().parse::<i64>().ok()?.to_string(),
            // Around the first separator (`WS_CBPOL.D.US`: `before:.` →
            // `WS_CBPOL`, `after:.` → `D.US`).
            f if f.starts_with("before:") => v.split_once(&f[7..])?.0.to_string(),
            f if f.starts_with("after:") => v.split_once(&f[6..])?.1.to_string(),
            _ => return None,
        };
    }
    Some(v)
}

/// Fills a template. A value that is the whole template goes in as it is
/// (`{{url}}`); inside an address it is percent-encoded so that it can
/// never change the host or add a path or a parameter (D942).
/// `Err(name)` on the first name `lookup` does not know.
pub fn fill(
    template: &str,
    address: bool,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let trimmed = template.trim();
    if let Some(name) = trimmed
        .strip_prefix("{{")
        .and_then(|r| r.strip_suffix("}}"))
        .filter(|n| !n.contains("{{") && !n.contains("}}"))
    {
        return value(name.trim(), &lookup).ok_or_else(|| name.trim().to_string());
    }
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else {
            out.push_str(&rest[i..]);
            return Ok(out);
        };
        let name = after[..j].trim();
        let value = value(name, &lookup).ok_or_else(|| name.to_string())?;
        if address {
            out.extend(
                url::form_urlencoded::byte_serialize(value.as_bytes()).map(|s| {
                    // `+` is a space only in a query; `%20` is one everywhere.
                    if s == "+" {
                        "%20"
                    } else {
                        s
                    }
                }),
            );
        } else {
            out.push_str(&value);
        }
        rest = &after[j + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The names a template uses, without their filters.
pub fn names(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find("{{") {
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else { break };
        let expr = &after[..j];
        out.push(expr.split('|').next().unwrap_or(expr).trim().to_string());
        rest = &after[j + 2..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str) -> Option<String> {
        match name {
            "pair" => Some("XAU/USD".into()),
            "year" => Some("2015".into()),
            "url" => Some("https://data.example.io/a?b=1".into()),
            _ => None,
        }
    }

    #[test]
    fn values_cannot_escape_the_address() {
        assert_eq!(
            fill("https://data.example.io/{{pair}}/{{year}}", true, row).unwrap(),
            "https://data.example.io/XAU%2FUSD/2015"
        );
        assert_eq!(
            fill("{{url}}", true, row).unwrap(),
            "https://data.example.io/a?b=1"
        );
        assert_eq!(fill("a {{pair}}", false, row).unwrap(), "a XAU/USD");
        assert_eq!(fill("x/{{nope}}", true, row), Err("nope".into()));
        assert_eq!(fill("/{{pair|lower}}", true, row).unwrap(), "/xau%2Fusd");
        let month = |n: &str| (n == "m").then(|| "03".to_string());
        assert_eq!(fill("{{m|int}}", false, month).unwrap(), "3");
        let code = |_: &str| Some("WS_CBPOL.D.US".to_string());
        assert_eq!(
            fill("/{{c|before:.}}/{{c|after:.}}/all", true, code).unwrap(),
            "/WS_CBPOL/D.US/all"
        );
        assert_eq!(fill("{{year|bogus}}", false, row), Err("year|bogus".into()));
    }

    #[test]
    fn names_are_listed() {
        assert_eq!(names("{{a}}/x/{{ b |upper}}"), ["a", "b"]);
    }
}
