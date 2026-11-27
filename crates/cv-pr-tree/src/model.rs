//! Flattened file tree of the changed files: folders first, single-child folder chains
//! compressed into one row ("src/app/features"), collapsed folders skipped.

use std::collections::{BTreeMap, HashSet};

use super::FolderMark;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TreeRow {
    Folder {
        path: String,
        label: String,
        depth: u16,
        file_count: u32,
        is_collapsed: bool,
        /// Filled in by [`super::FileTree::refresh`], which knows the viewed marks.
        mark: FolderMark,
    },
    /// `index` into the changed-file list.
    File { index: u32, depth: u16 },
}

#[derive(Default)]
struct Node {
    folders: BTreeMap<String, Node>,
    files: Vec<u32>,
    file_count: u32,
}

/// The rows of the files `paths` yields as (file index, path), and every file in the order the
/// rows would list them with no folder collapsed.
pub(crate) fn build_with_order<'a>(
    paths: impl Iterator<Item = (usize, &'a str)>,
    collapsed: &HashSet<String>,
) -> (Vec<TreeRow>, Vec<u32>) {
    let mut root = Node::default();
    for (index, path) in paths {
        let mut node = &mut root;
        node.file_count += 1;
        let folders = path.rsplit_once('/').map(|(folders, _)| folders);
        for segment in folders.into_iter().flat_map(|folders| folders.split('/')) {
            node = node.folders.entry(segment.to_owned()).or_default();
            node.file_count += 1;
        }
        node.files.push(index as u32);
    }
    let mut rows = Vec::new();
    emit(&root, "", 0, collapsed, &mut rows);
    let mut order = Vec::with_capacity(root.file_count as usize);
    collect_order(&root, &mut order);
    (rows, order)
}

/// Files in row order: each folder's subfolders first, then its own files.
fn collect_order(node: &Node, order: &mut Vec<u32>) {
    for child in node.folders.values() {
        collect_order(child, order);
    }
    order.extend_from_slice(&node.files);
}

fn emit(
    node: &Node,
    prefix: &str,
    depth: u16,
    collapsed: &HashSet<String>,
    rows: &mut Vec<TreeRow>,
) {
    for (name, child) in &node.folders {
        let mut label = name.clone();
        let mut path = join(prefix, name);
        let mut current = child;
        while current.files.is_empty() && current.folders.len() == 1 {
            let Some((next_name, next)) = current.folders.iter().next() else {
                break;
            };
            label = format!("{label}/{next_name}");
            path = join(&path, next_name);
            current = next;
        }
        let is_collapsed = collapsed.contains(&path);
        rows.push(TreeRow::Folder {
            path: path.clone(),
            label,
            depth,
            file_count: current.file_count,
            is_collapsed,
            mark: FolderMark::None,
        });
        if !is_collapsed {
            emit(current, &path, depth + 1, collapsed, rows);
        }
    }
    rows.extend(node.files.iter().map(|index| TreeRow::File { index: *index, depth }));
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        return name.to_owned();
    }
    format!("{prefix}/{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                TreeRow::Folder { label, depth, .. } => {
                    format!("{}{label}/", "  ".repeat(*depth as usize))
                }
                TreeRow::File { index, depth } => {
                    format!("{}#{index}", "  ".repeat(*depth as usize))
                }
            })
            .collect()
    }

    #[test]
    fn compresses_single_child_chains_and_lists_folders_first() {
        let paths = ["src/app/features/a.rs", "src/app/features/b.rs", "src/lib.rs", "README.md"];
        let (rows, _) = build_with_order(paths.iter().copied().enumerate(), &HashSet::new());
        assert_eq!(labels(&rows), ["src/", "  app/features/", "    #0", "    #1", "  #2", "#3"]);
        let TreeRow::Folder { file_count, .. } = &rows[0] else { panic!("folder expected") };
        assert_eq!(*file_count, 3);
    }

    #[test]
    fn skips_children_of_collapsed_folders() {
        let paths = ["src/a.rs", "docs/b.md"];
        let collapsed: HashSet<String> = ["src".to_owned()].into();
        let (rows, _) = build_with_order(paths.iter().copied().enumerate(), &collapsed);
        assert_eq!(labels(&rows), ["docs/", "  #1", "src/"]);
    }

    #[test]
    fn orders_every_file_as_the_open_tree_lists_them() {
        let paths = [
            "src/app/features/a.rs",
            "src/lib.rs",
            "README.md",
            "docs/b.md",
            "src/app/features/c.rs",
        ];
        let collapsed: HashSet<String> = ["src".to_owned()].into();
        let (rows, order) = build_with_order(paths.iter().copied().enumerate(), &collapsed);
        assert_eq!(order, [3, 0, 4, 1, 2], "collapsed folders keep their files in the order");
        let (open, _) = build_with_order(paths.iter().copied().enumerate(), &HashSet::new());
        let listed: Vec<u32> = open
            .iter()
            .filter_map(|row| match row {
                TreeRow::File { index, .. } => Some(*index),
                TreeRow::Folder { .. } => None,
            })
            .collect();
        assert_eq!(listed, order);
        assert!(rows.len() < open.len());
    }
}
