//! Canonical list pagination — delegates to the existing `Pager`
//! (`components/pager.rs`, `{count, …}` envelope D14) while standardizing the
//! wiring and the page size: the component sets `page.set(new_index)`
//! on its own (the resource re-fetches because it reads `page()` in its
//! synchronous part — doctrine), a single page size (D14), and it sits in
//! the list body (children of `ListStates`), never in `DataTable`.

use dioxus::prelude::*;

use crate::components::pager::Pager;

/// Default list page size (D14: `PAGINATION_DEFAULT_LIMIT`).
pub const PAGE_SIZE: i64 = 10;

#[component]
pub fn ListPager(
    /// Total returned by the `{count, …}` envelope.
    count: i64,
    /// Signal of the current page (0-based).
    mut page: Signal<i64>,
    /// Page size — D14 default (10). Can be overridden for denser
    /// card grids (catalog: 12).
    #[props(default = PAGE_SIZE)]
    page_size: i64,
) -> Element {
    rsx! {
        Pager {
            count,
            page_size,
            page,
            on_navigate: move |new_page| page.set(new_page),
        }
    }
}

/// Rows of page `page` (0-based) out of a fully loaded list.
// shortcut: client-side paging over `request_all`; move to server offsets if a list grows past a few hundred rows.
pub fn page_slice<T: Clone>(all: &[T], page: i64) -> Vec<T> {
    all.iter()
        .skip((page.max(0) * PAGE_SIZE) as usize)
        .take(PAGE_SIZE as usize)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_slice_bounds() {
        let all: Vec<i64> = (0..23).collect();
        assert_eq!(page_slice(&all, 0), (0..10).collect::<Vec<_>>());
        assert_eq!(page_slice(&all, 2), vec![20, 21, 22]);
        assert!(page_slice(&all, 5).is_empty());
    }
}
