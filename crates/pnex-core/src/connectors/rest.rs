//! The request of a `connector_query` step on a REST connector (connectors.md D793):
//! path, parameters, where the records sit, which columns to keep, how to page.
//! Pure logic only; the server sends the requests.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PARAMS_MAX: usize = 50;
pub const COLUMNS_MAX: usize = 500;
/// Pages one run reads at most, unless the step lowers it.
pub const PAGES_MAX: u32 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Paging {
    #[default]
    None,
    /// `page_param` = 1, 2, 3…
    Page,
    /// `page_param` = 0, size, 2 × size…
    Offset,
    /// The value at `cursor_pointer` of a page goes into `cursor_param` of the next.
    Cursor,
    /// The `Link: <…>; rel="next"` header.
    Link,
}

/// One output column: its name and a JSON pointer inside a record (`/address/city`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RestColumn {
    pub name: String,
    pub pointer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RestQuery {
    /// Appended to the connector's base path.
    #[serde(default)]
    pub path: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// JSON pointer to the array of records; empty = the body is the array.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub records: String,
    /// Empty = the top-level keys of the first record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<RestColumn>,
    #[serde(default)]
    pub paging: Paging,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub page_param: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub size_param: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cursor_pointer: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cursor_param: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pages: Option<u32>,
}

fn pointer_ok(p: &str) -> bool {
    p.is_empty() || (p.starts_with('/') && p.len() <= 500)
}

fn param_ok(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 100
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '[' | ']'))
}

impl RestQuery {
    pub fn max_pages(&self) -> u32 {
        self.max_pages.unwrap_or(PAGES_MAX).min(PAGES_MAX)
    }

    /// What is wrong, as a short English message for the editor.
    pub fn check(&self) -> Option<&'static str> {
        if !self.path.is_empty() && !super::clean_path(&self.path) {
            return Some("the path must start with / and hold no query or fragment");
        }
        if self.params.len() > PARAMS_MAX
            || self
                .params
                .iter()
                .any(|(k, v)| !param_ok(k) || v.len() > 2048)
        {
            return Some("at most 50 parameters, names of letters, digits, _ - . [ ]");
        }
        if !pointer_ok(&self.records) || !pointer_ok(&self.cursor_pointer) {
            return Some("a JSON pointer starts with /");
        }
        if self.columns.len() > COLUMNS_MAX
            || self
                .columns
                .iter()
                .any(|c| c.name.trim().is_empty() || c.name.len() > 200 || !pointer_ok(&c.pointer))
        {
            return Some("at most 500 columns, each a name and a JSON pointer");
        }
        if self.max_pages == Some(0) || self.page_size == Some(0) {
            return Some("max_pages and page_size must be positive");
        }
        let need = |p: &str| p.is_empty() || !param_ok(p);
        match self.paging {
            Paging::Page | Paging::Offset if need(&self.page_param) => {
                Some("page paging needs a page_param")
            }
            Paging::Offset if self.page_size.is_none() => Some("offset paging needs a page_size"),
            Paging::Cursor if self.cursor_pointer.is_empty() || need(&self.cursor_param) => {
                Some("cursor paging needs a cursor_pointer and a cursor_param")
            }
            _ if !self.size_param.is_empty() && !param_ok(&self.size_param) => {
                Some("size_param holds letters, digits, _ - . [ ]")
            }
            _ => None,
        }
    }

    /// Query parameters of page `n` (0-based), `cursor` from the previous page.
    pub fn page_params(&self, n: u32, cursor: Option<&str>) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .params
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut set = |k: &str, v: String| {
            out.retain(|(x, _)| x != k);
            out.push((k.to_string(), v));
        };
        if let (false, Some(size)) = (self.size_param.is_empty(), self.page_size) {
            set(&self.size_param, size.to_string());
        }
        match self.paging {
            Paging::Page => set(&self.page_param, (n + 1).to_string()),
            Paging::Offset => {
                let size = self.page_size.unwrap_or(1) as u64;
                set(&self.page_param, (n as u64 * size).to_string());
            }
            Paging::Cursor => {
                if let Some(c) = cursor {
                    set(&self.cursor_param, c.to_string());
                }
            }
            Paging::None | Paging::Link => {}
        }
        out
    }

    /// The records of a page; `None` when the pointer finds no array.
    pub fn records<'a>(&self, body: &'a Value) -> Option<&'a Vec<Value>> {
        body.pointer(&self.records).and_then(Value::as_array)
    }

    /// The cursor for the next page, if the page carries one.
    pub fn next_cursor(&self, body: &Value) -> Option<String> {
        match body.pointer(&self.cursor_pointer)? {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }

    /// Columns to write: the declared ones, or the keys of the first record.
    pub fn columns_for(&self, first: Option<&Value>) -> Vec<RestColumn> {
        if !self.columns.is_empty() {
            return self.columns.clone();
        }
        let Some(Value::Object(map)) = first else {
            return vec![RestColumn {
                name: "value".into(),
                pointer: String::new(),
            }];
        };
        map.keys()
            .map(|k| RestColumn {
                name: k.clone(),
                pointer: format!("/{}", k.replace('~', "~0").replace('/', "~1")),
            })
            .collect()
    }
}

/// One cell: a scalar as text, null or absent as empty, anything else as JSON.
pub fn cell(record: &Value, pointer: &str) -> String {
    match record.pointer(pointer) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(other) => other.to_string(),
    }
}

/// One CSV line (RFC 4180), with its line break.
pub fn csv_line<I: IntoIterator<Item = S>, S: AsRef<str>>(cells: I) -> String {
    let mut out = String::new();
    for (i, c) in cells.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let c = c.as_ref();
        if c.contains([',', '"', '\n', '\r']) {
            out.push('"');
            out.push_str(&c.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(c);
        }
    }
    out.push_str("\r\n");
    out
}

/// The `rel="next"` target of a `Link` header.
pub fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        let next = params.split(';').any(|p| {
            let p = p.trim().replace(' ', "");
            p == "rel=\"next\"" || p == "rel=next"
        });
        let url = url.trim().strip_prefix('<')?.strip_suffix('>')?;
        next.then(|| url.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn q() -> RestQuery {
        RestQuery {
            path: "/items".into(),
            records: "/data".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_query_is_checked() {
        assert!(q().check().is_none());
        let bad = [
            RestQuery {
                path: "/x?y=1".into(),
                ..q()
            },
            RestQuery {
                records: "data".into(),
                ..q()
            },
            RestQuery {
                paging: Paging::Page,
                ..q()
            },
            RestQuery {
                paging: Paging::Offset,
                page_param: "offset".into(),
                ..q()
            },
            RestQuery {
                paging: Paging::Cursor,
                cursor_param: "after".into(),
                ..q()
            },
            RestQuery {
                params: [("a b".to_string(), "1".to_string())].into(),
                ..q()
            },
            RestQuery {
                max_pages: Some(0),
                ..q()
            },
        ];
        for b in bad {
            assert!(b.check().is_some(), "{b:?}");
        }
    }

    #[test]
    fn pages_set_their_parameter() {
        let page = RestQuery {
            paging: Paging::Page,
            page_param: "page".into(),
            size_param: "per_page".into(),
            page_size: Some(50),
            params: [("page".to_string(), "9".to_string())].into(),
            ..q()
        };
        let p = page.page_params(2, None);
        assert!(p.contains(&("page".into(), "3".into())));
        assert!(p.contains(&("per_page".into(), "50".into())));
        assert_eq!(p.iter().filter(|(k, _)| k == "page").count(), 1);

        let offset = RestQuery {
            paging: Paging::Offset,
            page_param: "offset".into(),
            page_size: Some(20),
            ..q()
        };
        assert_eq!(
            offset.page_params(3, None),
            vec![("offset".into(), "60".into())]
        );

        let cursor = RestQuery {
            paging: Paging::Cursor,
            cursor_pointer: "/next".into(),
            cursor_param: "after".into(),
            ..q()
        };
        assert!(cursor.page_params(0, None).is_empty());
        assert_eq!(
            cursor.page_params(1, Some("abc")),
            vec![("after".into(), "abc".into())]
        );
        assert_eq!(
            cursor.next_cursor(&json!({"next": "abc"})).as_deref(),
            Some("abc")
        );
        assert_eq!(
            cursor.next_cursor(&json!({"next": 42})).as_deref(),
            Some("42")
        );
        assert_eq!(cursor.next_cursor(&json!({"next": null})), None);
        assert_eq!(cursor.next_cursor(&json!({"next": ""})), None);
    }

    #[test]
    fn records_become_csv_rows() {
        let body = json!({"data": [{"id": 1, "name": "Acme, Inc", "tags": ["a"], "a/b": true, "x": null}]});
        let q = q();
        let recs = q.records(&body).unwrap();
        // Key order depends on serde_json features: compare sorted.
        let mut cols = q.columns_for(recs.first());
        cols.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<_> = cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a/b", "id", "name", "tags", "x"]);
        let row: Vec<String> = cols.iter().map(|c| cell(&recs[0], &c.pointer)).collect();
        assert_eq!(csv_line(&row), "true,1,\"Acme, Inc\",\"[\"\"a\"\"]\",\r\n");
        assert!(q.records(&json!({"data": {}})).is_none());
        let root = RestQuery::default();
        assert_eq!(root.records(&json!([1, 2])).map(Vec::len), Some(2));
        assert_eq!(root.columns_for(Some(&json!(1)))[0].name, "value");
        assert_eq!(cell(&json!(7), ""), "7");
    }

    #[test]
    fn the_next_link_is_found() {
        let h = r#"<https://api.example.org/items?page=1>; rel="prev", <https://api.example.org/items?page=3>; rel="next""#;
        assert_eq!(
            next_link(h).as_deref(),
            Some("https://api.example.org/items?page=3")
        );
        assert_eq!(next_link(r#"<https://x/a>; rel="last""#), None);
        assert_eq!(next_link("garbage"), None);
    }
}
