//! Standardised CRUD foundation (lists) — the "list of a resource" pattern
//! written **only once** and composed everywhere: layout (header, back,
//! "add" action, filter slot), state zones (loading / empty /
//! error), generic table, filter bar, modal form.
//!
//! Creating a new list page = composing `ListLayout` + `FilterBar` +
//! `ListStates` + `DataTable` + `Pager` — without ever rewriting the
//! header/back/add/filters or the states. Full doctrine and
//! migration checklist: see the module's `README.md`.

pub mod filters;
pub mod form;
pub mod layout;
pub mod pager;
pub mod states;
pub mod table;
