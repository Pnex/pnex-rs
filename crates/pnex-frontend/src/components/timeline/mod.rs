//! One timeline for everything that happens over time (design-system.md
//! §6 rule 9, D220): a ruler with the density of what happens and folded
//! empty stretches, lanes of labelled marks or covered spans, and one
//! cursor across them. Pages map their objects to [`Lane`]s and handle the
//! gestures; the playback row stays the shared transport
//! (`components::transport`).

pub mod axis;
mod view;

pub use axis::TimeBase;
pub use view::{Body, Hover, Item, ItemKind, Lane, Timeline};
