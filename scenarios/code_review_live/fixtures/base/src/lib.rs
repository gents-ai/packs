//! Pagination helpers.

/// Number of pages needed for `total` items at `per_page` items per page.
/// `per_page` must be at least 1.
pub fn page_count(total: usize, per_page: usize) -> usize {
    (total + per_page - 1) / per_page
}

/// The items on 1-based page `page`, in order.
pub fn page_items<T: Clone>(items: &[T], page: usize, per_page: usize) -> Vec<T> {
    let start = (page - 1) * per_page;
    let end = (start + per_page).min(items.len());
    items[start..end].to_vec()
}
