//! Masking by classification (crm.md D443): the properties, link
//! attributes and titles a caller's role may not read are removed from what
//! the ontology reads return. Pure functions on the org schema; the
//! controllers build a [`Redaction`] from the principal.

use pnex_core::ontology::api::{
    ChangeFeed, ChangeView, HierarchyView, LinkView, Neighborhood, ObjectRef, ObjectView,
    QueryResult,
};
use pnex_core::ontology::schema::Classification;
use serde_json::{Map, Value};

use super::OrgSchema;

/// Read permission of personal data (D443).
pub const PERSONAL_READ: &str = "ontology.personal:read";
/// Read permission of sensitive data (D443).
pub const SENSITIVE_READ: &str = "ontology.sensitive:read";

/// Key listed in `redacted` when the title is hidden.
pub const TITLE: &str = "title";

/// What the caller may not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Redaction {
    pub personal: bool,
    pub sensitive: bool,
}

impl Redaction {
    /// Nothing hidden.
    pub const NONE: Self = Self {
        personal: false,
        sensitive: false,
    };
    /// Everything classified hidden: the history of an erased object (D446).
    pub const ALL: Self = Self {
        personal: true,
        sensitive: true,
    };

    /// From the caller's permissions.
    pub fn of(can: impl Fn(&str) -> bool) -> Self {
        Self {
            personal: !can(PERSONAL_READ),
            sensitive: !can(SENSITIVE_READ),
        }
    }

    pub fn is_none(&self) -> bool {
        !self.personal && !self.sensitive
    }

    pub fn hides(&self, c: Classification) -> bool {
        match c {
            Classification::Personnelle => self.personal,
            Classification::Sensible => self.sensitive,
            Classification::Publique | Classification::Interne => false,
        }
    }

    fn hides_title(&self, s: &OrgSchema, type_key: &str) -> bool {
        s.object_type(type_key)
            .is_some_and(|t| self.hides(t.title_classification))
    }

    /// Types whose title the caller may not read: a query never searches
    /// nor sorts on it (else the result would reveal it).
    pub fn title_hidden_types(&self, s: &OrgSchema) -> Vec<String> {
        if self.is_none() {
            return Vec::new();
        }
        s.types
            .iter()
            .map(|(t, _, _)| t)
            .filter(|t| self.hides(t.title_classification))
            .map(|t| t.key.clone())
            .collect()
    }

    /// Types where the property `key` is hidden from the caller: a query
    /// never filters nor sorts on it there.
    pub fn property_hidden_types(&self, s: &OrgSchema, key: &str) -> Vec<String> {
        if self.is_none() {
            return Vec::new();
        }
        s.types
            .iter()
            .map(|(t, _, _)| t)
            .filter(|t| {
                t.properties
                    .iter()
                    .any(|p| p.key == key && self.hides(p.classification))
            })
            .map(|t| t.key.clone())
            .collect()
    }

    /// Removes the hidden properties of an object of `type_key`; returns
    /// the removed keys.
    fn properties(
        &self,
        s: &OrgSchema,
        type_key: &str,
        props: &mut Map<String, Value>,
    ) -> Vec<String> {
        let Some(def) = s.object_type(type_key) else {
            return Vec::new();
        };
        let hidden: Vec<String> = def
            .properties
            .iter()
            .filter(|p| self.hides(p.classification) && props.contains_key(&p.key))
            .map(|p| p.key.clone())
            .collect();
        for k in &hidden {
            props.remove(k);
        }
        hidden
    }

    pub fn object(&self, s: &OrgSchema, v: &mut ObjectView) {
        if self.is_none() {
            return;
        }
        let mut hidden = self.properties(s, &v.type_key, &mut v.properties);
        if self.hides_title(s, &v.type_key) {
            v.title = masked(&v.id);
            hidden.push(TITLE.into());
        }
        v.redacted = hidden;
    }

    pub fn object_ref(&self, s: &OrgSchema, r: &mut ObjectRef) {
        if !self.is_none() && self.hides_title(s, &r.type_key) {
            r.title = masked(&r.id);
        }
    }

    pub fn link(&self, s: &OrgSchema, l: &mut LinkView) {
        if self.is_none() {
            return;
        }
        if let Some(def) = s.link_type(&l.link_type) {
            for a in &def.attributes {
                if self.hides(a.classification) {
                    l.attributes.remove(&a.key);
                }
            }
        }
        self.object_ref(s, &mut l.source);
        self.object_ref(s, &mut l.target);
    }

    pub fn refs(&self, s: &OrgSchema, refs: &mut [ObjectRef]) {
        for r in refs {
            self.object_ref(s, r);
        }
    }

    pub fn neighborhood(&self, s: &OrgSchema, n: &mut Neighborhood) {
        self.refs(s, &mut n.nodes);
        for l in &mut n.links {
            self.link(s, l);
        }
    }

    /// Node titles; the other parents named in `also_under` are masked
    /// when the tree is built (`hierarchy::tree`).
    pub fn hierarchy(&self, s: &OrgSchema, h: &mut HierarchyView) {
        if self.is_none() {
            return;
        }
        for n in &mut h.nodes {
            if self.hides_title(s, &n.type_key) {
                n.title = masked(&n.id);
            }
        }
    }

    pub fn feed(&self, s: &OrgSchema, f: &mut ChangeFeed) {
        for e in &mut f.changes {
            if let Some(o) = &mut e.object {
                self.object(s, o);
            }
            if let Some(l) = &mut e.link {
                self.link(s, l);
            }
        }
    }

    pub fn query(&self, s: &OrgSchema, r: &mut QueryResult) {
        for row in &mut r.rows {
            self.object(s, &mut row.object);
        }
    }

    /// A journal entry: `before` / `after` hold an object or a link view.
    pub fn change(&self, s: &OrgSchema, c: &mut ChangeView) {
        if self.is_none() {
            return;
        }
        for doc in [&mut c.before, &mut c.after] {
            self.doc(s, doc);
        }
    }

    /// Any JSON made of object views, references, links and journal
    /// entries (the case reads, D443): every map naming an object (`id` +
    /// `type_key`) or a link (`link_type`) is masked, at every depth.
    pub fn json(&self, s: &OrgSchema, v: &mut Value) {
        if self.is_none() {
            return;
        }
        match v {
            Value::Array(items) => {
                for x in items {
                    self.json(s, x);
                }
            }
            Value::Object(o) => {
                let lt = o.get("link_type").and_then(Value::as_str).map(String::from);
                if let (Some(def), Some(Value::Object(attrs))) =
                    (lt.and_then(|k| s.link_type(&k)), o.get_mut("attributes"))
                {
                    for a in &def.attributes {
                        if self.hides(a.classification) {
                            attrs.remove(&a.key);
                        }
                    }
                }
                let key = |k: &str| o.get(k).and_then(Value::as_str).map(String::from);
                if let (Some(t), Some(id)) = (key("type_key"), key("id")) {
                    if let Some(Value::Object(props)) = o.get_mut("properties") {
                        self.properties(s, &t, props);
                    }
                    if self.hides_title(s, &t) && o.contains_key(TITLE) {
                        o.insert(TITLE.into(), Value::String(masked(&id)));
                    }
                }
                for x in o.values_mut() {
                    self.json(s, x);
                }
            }
            _ => {}
        }
    }

    fn doc(&self, s: &OrgSchema, doc: &mut Value) {
        if doc.get("link_type").is_some() {
            if let Ok(mut l) = serde_json::from_value::<LinkView>(doc.clone()) {
                self.link(s, &mut l);
                *doc = serde_json::to_value(l).unwrap_or(Value::Null);
            }
            return;
        }
        let Some(type_key) = doc
            .get("type_key")
            .and_then(Value::as_str)
            .map(String::from)
        else {
            return;
        };
        let id = doc
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let title_hidden = self.hides_title(s, &type_key);
        let Some(o) = doc.as_object_mut() else {
            return;
        };
        if let Some(Value::Object(props)) = o.get_mut("properties") {
            self.properties(s, &type_key, props);
        }
        if title_hidden && o.contains_key(TITLE) {
            o.insert(TITLE.into(), Value::String(masked(&id)));
        }
    }
}

/// The title shown in place of a hidden one: neutral, translated by
/// nobody, still telling two objects apart.
pub fn masked(id: &str) -> String {
    format!("•••• {}", short(id))
}

/// First 8 characters of an id.
pub fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::ontology::api::QueryRow;
    use pnex_core::ontology::{LinkTypeDef, ObjectTypeDef};
    use serde_json::json;

    fn schema() -> OrgSchema {
        let person: ObjectTypeDef = serde_json::from_value(json!({
            "key": "person", "name": "Person", "title_classification": "personnelle",
            "properties": [
                {"key": "notes", "kind": "text", "classification": "personnelle"},
                {"key": "health", "kind": "text", "classification": "sensible"},
                {"key": "wikidata", "kind": "text"}
            ]
        }))
        .unwrap();
        let org: ObjectTypeDef =
            serde_json::from_value(json!({"key": "organization", "name": "Org"})).unwrap();
        let employs: LinkTypeDef = serde_json::from_value(json!({
            "key": "employs", "from_types": ["organization"], "to_types": ["person"],
            "attributes": [
                {"key": "position", "kind": "text", "classification": "personnelle"},
                {"key": "source", "kind": "text"}
            ]
        }))
        .unwrap();
        OrgSchema {
            types: vec![(person, 1, None), (org, 1, None)],
            links: vec![(employs, 1, None)],
        }
    }

    fn ann() -> ObjectView {
        serde_json::from_value(json!({
            "id": "1a2b3c4d-0000-0000-0000-000000000000", "type_key": "person", "title": "Ann Smith",
            "native_id": "", "system": false, "version": 1, "valid_from": "", "created_at": "", "updated_at": "",
            "properties": {"notes": "met at the fair", "health": "x", "wikidata": "Q1"}
        }))
        .unwrap()
    }

    #[test]
    fn a_viewer_without_personal_read_sees_a_masked_person() {
        let s = schema();
        let r = Redaction::of(|p| p == SENSITIVE_READ);
        let mut v = ann();
        r.object(&s, &mut v);
        assert_eq!(v.title, "•••• 1a2b3c4d");
        assert_eq!(
            v.properties,
            json!({"health": "x", "wikidata": "Q1"})
                .as_object()
                .cloned()
                .unwrap()
        );
        assert_eq!(v.redacted, vec!["notes".to_string(), TITLE.into()]);
    }

    #[test]
    fn full_rights_change_nothing() {
        let s = schema();
        let mut v = ann();
        Redaction::of(|_| true).object(&s, &mut v);
        assert_eq!(v, ann());
    }

    #[test]
    fn sensitive_hidden_alone_keeps_the_name() {
        let s = schema();
        let mut v = ann();
        Redaction::of(|p| p == PERSONAL_READ).object(&s, &mut v);
        assert_eq!(v.title, "Ann Smith");
        assert_eq!(v.redacted, vec!["health".to_string()]);
    }

    #[test]
    fn links_lose_personal_attributes_and_masked_ends() {
        let s = schema();
        let mut l: LinkView = serde_json::from_value(json!({
            "id": 1, "link_type": "employs", "valid_from": "",
            "source": {"id": "org-1", "type_key": "organization", "title": "Acme"},
            "target": {"id": "1a2b3c4d-9", "type_key": "person", "title": "Ann Smith"},
            "attributes": {"position": "CEO", "source": "registry"}
        }))
        .unwrap();
        Redaction::ALL.link(&s, &mut l);
        assert_eq!(
            l.attributes,
            json!({"source": "registry"}).as_object().cloned().unwrap()
        );
        assert_eq!(l.source.title, "Acme");
        assert_eq!(l.target.title, "•••• 1a2b3c4d");
    }

    #[test]
    fn journal_entries_are_redacted_on_both_sides() {
        let s = schema();
        let before = serde_json::to_value(ann()).unwrap();
        let mut c = ChangeView {
            at: String::new(),
            action: "object.update".into(),
            source_ref: String::new(),
            before: before.clone(),
            after: before,
        };
        Redaction::ALL.change(&s, &mut c);
        for doc in [&c.before, &c.after] {
            assert_eq!(doc["title"], "•••• 1a2b3c4d");
            assert_eq!(doc["properties"], json!({"wikidata": "Q1"}));
        }
    }

    #[test]
    fn query_rows_are_redacted() {
        let s = schema();
        let mut r = QueryResult {
            count: 1,
            rows: vec![QueryRow {
                object: ann(),
                latest: Default::default(),
            }],
        };
        Redaction::ALL.query(&s, &mut r);
        assert_eq!(r.rows[0].object.title, "•••• 1a2b3c4d");
    }
}
