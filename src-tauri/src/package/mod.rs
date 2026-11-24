mod export;
mod import;
#[cfg(test)]
mod tests;

use std::collections::{HashSet, VecDeque};

pub use export::export_partial_package;
pub use import::import_partial_package;

pub(super) struct RawFolderRow {
    pub(super) id: String,
    pub(super) parent_id: Option<String>,
    pub(super) name: String,
    pub(super) color: String,
    pub(super) sort_order: i32,
    pub(super) created_at: i64,
    pub(super) updated_at: i64,
}

pub(super) fn topological_folders(
    rows: Vec<RawFolderRow>,
    allowed: Option<&HashSet<String>>,
) -> Vec<RawFolderRow> {
    let mut pending: VecDeque<RawFolderRow> = rows.into_iter().collect();
    let mut placed = HashSet::new();
    let mut sorted = Vec::with_capacity(pending.len());
    let mut guard = 0usize;
    let max_iters = pending.len().saturating_mul(pending.len() + 2).max(16);

    while let Some(row) = pending.pop_front() {
        guard += 1;
        let parent_ready = match &row.parent_id {
            None => true,
            Some(pid) => {
                if let Some(allow_set) = allowed {
                    !allow_set.contains(pid) || placed.contains(pid)
                } else {
                    placed.contains(pid)
                }
            }
        };

        if parent_ready || guard > max_iters {
            placed.insert(row.id.clone());
            sorted.push(row);
        } else {
            pending.push_back(row);
        }
    }
    sorted
}
