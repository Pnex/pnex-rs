//! Socle CRUD standardisé (listes) — le motif « liste d'une ressource »
//! écrit **une seule fois** et composé partout : layout (header, retour,
//! action « ajouter », slot filtres), zones d'état (loading / empty /
//! error), table générique, barre de filtres, formulaire modal.
//!
//! Créer une nouvelle page liste = composer `ListLayout` + `FilterBar` +
//! `ListStates` + `DataTable` + `Pager` — sans jamais réécrire le
//! header/retour/ajouter/filtres ni les états. Doctrine complète et
//! checklist de migration : voir `README.md` du module.

pub mod filters;
pub mod form;
pub mod layout;
pub mod pager;
pub mod states;
pub mod table;
