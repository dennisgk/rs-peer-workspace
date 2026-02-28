use std::fs;

use rs_peer_workspace_shared::app::DirectoryEntry;

use super::types::TreeEntry;

pub fn tree_from_entry(entry: DirectoryEntry) -> TreeEntry {
    TreeEntry {
        name: entry.name,
        path: entry.path,
        is_dir: entry.is_dir,
    }
}

pub fn list_local_directory(path: &str) -> anyhow::Result<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        entries.push(TreeEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: entry.path().to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
        });
    }
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(entries)
}
