//! Hierarchical Favorites Library with unlimited nested folders & subfolders.
//!
//! Persisted cleanly inside `micropdf.ini` (and automatically encrypted when
//! `File -> Encrypt Portable Config (micropdf.ini)` is enabled).

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FavNode {
    Folder {
        id: u64,
        name: String,
        expanded: bool,
        children: Vec<FavNode>,
    },
    Doc {
        id: u64,
        title: String,
        path: PathBuf,
    },
}

impl FavNode {
    pub(crate) fn id(&self) -> u64 {
        match self {
            Self::Folder { id, .. } | Self::Doc { id, .. } => *id,
        }
    }

    pub(crate) fn is_folder(&self) -> bool {
        matches!(self, Self::Folder { .. })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FavFlatRow {
    pub(crate) id: u64,
    pub(crate) depth: usize,
    pub(crate) is_folder: bool,
    pub(crate) expanded: bool,
    pub(crate) title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FavoritesTree {
    pub(crate) root: Vec<FavNode>,
    next_id: u64,
}

impl Default for FavoritesTree {
    fn default() -> Self {
        Self {
            root: Vec::new(),
            next_id: 1,
        }
    }
}

impl FavoritesTree {
    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        id
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.root.is_empty()
    }

    pub(crate) fn is_starred(&self, path: &Path) -> bool {
        if path.as_os_str().is_empty() {
            return false;
        }
        Self::contains_path(&self.root, path)
    }

    fn contains_path(nodes: &[FavNode], path: &Path) -> bool {
        for n in nodes {
            match n {
                FavNode::Doc { path: p, .. } if p == path => return true,
                FavNode::Folder { children, .. } if Self::contains_path(children, path) => return true,
                _ => {}
            }
        }
        false
    }

    pub(crate) fn all_docs(&self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        fn walk(nodes: &[FavNode], out: &mut Vec<(String, PathBuf)>) {
            for n in nodes {
                match n {
                    FavNode::Doc { title, path, .. } => out.push((title.clone(), path.clone())),
                    FavNode::Folder { children, .. } => walk(children, out),
                }
            }
        }
        walk(&self.root, &mut out);
        out
    }

    pub(crate) fn find_node(&self, id: u64) -> Option<&FavNode> {
        Self::find_node_in(&self.root, id)
    }

    fn find_node_in(nodes: &[FavNode], id: u64) -> Option<&FavNode> {
        for n in nodes {
            if n.id() == id {
                return Some(n);
            }
            if let FavNode::Folder { children, .. } = n {
                if let Some(found) = Self::find_node_in(children, id) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Resolves `selected_id` to the target folder ID for inserting new items:
    /// if `selected_id` is a folder, returns `Some(folder_id)`; if it's a doc, returns its parent folder; else `None` (root).
    pub(crate) fn target_folder_for_selection(&self, selected_id: Option<u64>) -> Option<u64> {
        let sid = selected_id?;
        if let Some(node) = self.find_node(sid) {
            if node.is_folder() {
                return Some(sid);
            }
        }
        Self::find_parent_folder(&self.root, sid, None)
    }

    fn find_parent_folder(nodes: &[FavNode], target_id: u64, current_parent: Option<u64>) -> Option<u64> {
        for n in nodes {
            if n.id() == target_id {
                return current_parent;
            }
            if let FavNode::Folder { id, children, .. } = n {
                if let Some(found) = Self::find_parent_folder(children, target_id, Some(*id)) {
                    return Some(found);
                }
            }
        }
        None
    }

    pub(crate) fn add_folder(&mut self, parent_folder_id: Option<u64>, name: String) -> u64 {
        let clean = name.trim();
        let folder_name = if clean.is_empty() {
            "New Folder".to_string()
        } else {
            clean.to_string()
        };
        let id = self.alloc_id();
        let node = FavNode::Folder {
            id,
            name: folder_name,
            expanded: true,
            children: Vec::new(),
        };
        if let Some(pid) = parent_folder_id {
            if Self::insert_into_folder(&mut self.root, pid, node.clone()) {
                return id;
            }
        }
        self.root.push(node);
        id
    }

    pub(crate) fn add_doc(&mut self, parent_folder_id: Option<u64>, title: String, path: PathBuf) -> u64 {
        let clean_title = if title.trim().is_empty() {
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("Document")
                .to_string()
        } else {
            title.trim().to_string()
        };

        // Deduplicate if already present in target folder
        if let Some(pid) = parent_folder_id {
            if let Some(existing) = Self::find_doc_in_folder(&self.root, pid, &path) {
                return existing;
            }
        } else if let Some(existing) = self.root.iter().find_map(|n| match n {
            FavNode::Doc { id, path: p, .. } if p == &path => Some(*id),
            _ => None,
        }) {
            return existing;
        }

        let id = self.alloc_id();
        let node = FavNode::Doc {
            id,
            title: clean_title,
            path,
        };
        if let Some(pid) = parent_folder_id {
            if Self::insert_into_folder(&mut self.root, pid, node.clone()) {
                return id;
            }
        }
        self.root.push(node);
        id
    }

    fn find_doc_in_folder(nodes: &[FavNode], folder_id: u64, path: &Path) -> Option<u64> {
        for n in nodes {
            if let FavNode::Folder { id, children, .. } = n {
                if *id == folder_id {
                    return children.iter().find_map(|c| match c {
                        FavNode::Doc { id: cid, path: p, .. } if p == path => Some(*cid),
                        _ => None,
                    });
                }
                if let Some(found) = Self::find_doc_in_folder(children, folder_id, path) {
                    return Some(found);
                }
            }
        }
        None
    }

    fn insert_into_folder(nodes: &mut [FavNode], folder_id: u64, item: FavNode) -> bool {
        for n in nodes {
            if let FavNode::Folder { id, expanded, children, .. } = n {
                if *id == folder_id {
                    *expanded = true;
                    children.push(item);
                    return true;
                }
                if Self::insert_into_folder(children, folder_id, item.clone()) {
                    return true;
                }
            }
        }
        false
    }

    pub(crate) fn toggle_expand(&mut self, folder_id: u64) -> bool {
        Self::toggle_expand_in(&mut self.root, folder_id)
    }

    fn toggle_expand_in(nodes: &mut [FavNode], folder_id: u64) -> bool {
        for n in nodes {
            if let FavNode::Folder { id, expanded, children, .. } = n {
                if *id == folder_id {
                    *expanded = !*expanded;
                    return true;
                }
                if Self::toggle_expand_in(children, folder_id) {
                    return true;
                }
            }
        }
        false
    }

    pub(crate) fn rename_node(&mut self, target_id: u64, new_name: String) -> bool {
        let clean = new_name.trim();
        if clean.is_empty() {
            return false;
        }
        Self::rename_in(&mut self.root, target_id, clean)
    }

    fn rename_in(nodes: &mut [FavNode], target_id: u64, new_name: &str) -> bool {
        for n in nodes {
            match n {
                FavNode::Folder { id, name, children, .. } => {
                    if *id == target_id {
                        *name = new_name.to_string();
                        return true;
                    }
                    if Self::rename_in(children, target_id, new_name) {
                        return true;
                    }
                }
                FavNode::Doc { id, title, .. } => {
                    if *id == target_id {
                        *title = new_name.to_string();
                        return true;
                    }
                }
            }
        }
        false
    }

    pub(crate) fn delete_node(&mut self, target_id: u64) -> bool {
        Self::take_node(&mut self.root, target_id).is_some()
    }

    fn take_node(nodes: &mut Vec<FavNode>, target_id: u64) -> Option<FavNode> {
        if let Some(pos) = nodes.iter().position(|n| n.id() == target_id) {
            return Some(nodes.remove(pos));
        }
        for n in nodes.iter_mut() {
            if let FavNode::Folder { children, .. } = n {
                if let Some(found) = Self::take_node(children, target_id) {
                    return Some(found);
                }
            }
        }
        None
    }

    fn is_descendant(node: &FavNode, candidate_id: u64) -> bool {
        if node.id() == candidate_id {
            return true;
        }
        if let FavNode::Folder { children, .. } = node {
            return children.iter().any(|c| Self::is_descendant(c, candidate_id));
        }
        false
    }

    pub(crate) fn move_node(&mut self, node_id: u64, target_folder_id: Option<u64>) -> bool {
        if let Some(tfid) = target_folder_id {
            if tfid == node_id {
                return false;
            }
            if let Some(src) = self.find_node(node_id) {
                if Self::is_descendant(src, tfid) {
                    return false;
                }
            } else {
                return false;
            }
        }
        let Some(node) = Self::take_node(&mut self.root, node_id) else {
            return false;
        };
        if let Some(tfid) = target_folder_id {
            if Self::insert_into_folder(&mut self.root, tfid, node.clone()) {
                return true;
            }
        }
        self.root.push(node);
        true
    }

    pub(crate) fn flatten_visible(&self, query: &str) -> Vec<FavFlatRow> {
        let q = query.trim().to_ascii_lowercase();
        let mut out = Vec::new();
        Self::flatten_in(&self.root, 0, &q, &mut out);
        out
    }

    fn subtree_matches(node: &FavNode, q: &str) -> bool {
        match node {
            FavNode::Doc { title, path, .. } => {
                title.to_ascii_lowercase().contains(q)
                    || path.to_string_lossy().to_ascii_lowercase().contains(q)
            }
            FavNode::Folder { name, children, .. } => {
                name.to_ascii_lowercase().contains(q)
                    || children.iter().any(|c| Self::subtree_matches(c, q))
            }
        }
    }

    fn flatten_in(nodes: &[FavNode], depth: usize, q: &str, out: &mut Vec<FavFlatRow>) {
        for n in nodes {
            if !q.is_empty() && !Self::subtree_matches(n, q) {
                continue;
            }
            match n {
                FavNode::Folder { id, name, expanded, children } => {
                    let show_children = *expanded || !q.is_empty();
                    out.push(FavFlatRow {
                        id: *id,
                        depth,
                        is_folder: true,
                        expanded: show_children,
                        title: name.clone(),
                    });
                    if show_children {
                        Self::flatten_in(children, depth + 1, q, out);
                    }
                }
                FavNode::Doc { id, title, .. } => {
                    out.push(FavFlatRow {
                        id: *id,
                        depth,
                        is_folder: false,
                        expanded: false,
                        title: title.clone(),
                    });
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // INI Persistence
    // -----------------------------------------------------------------------

    fn escape_ini_field(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('|', "\\p")
            .replace('\n', " ")
            .replace('\r', "")
    }

    fn unescape_ini_field(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('p') => out.push('|'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    fn split_escaped_pipes(s: &str, max_parts: usize) -> Vec<String> {
        let mut parts = Vec::new();
        let mut cur = String::new();
        let mut chars = s.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '|' && parts.len() + 1 < max_parts {
                parts.push(Self::unescape_ini_field(&cur));
                cur.clear();
            } else if ch == '\\' {
                cur.push('\\');
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            } else {
                cur.push(ch);
            }
        }
        parts.push(Self::unescape_ini_field(&cur));
        parts
    }

    pub(crate) fn to_ini_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        Self::write_ini_nodes(&self.root, 0, &mut out);
        out
    }

    fn write_ini_nodes(nodes: &[FavNode], parent_id: u64, out: &mut Vec<String>) {
        for n in nodes {
            match n {
                FavNode::Folder { id, name, expanded, children } => {
                    out.push(format!(
                        "fav_folder={}|{}|{}|{}",
                        id,
                        parent_id,
                        if *expanded { 1 } else { 0 },
                        Self::escape_ini_field(name)
                    ));
                    Self::write_ini_nodes(children, *id, out);
                }
                FavNode::Doc { id, title, path } => {
                    let p_str = path.to_string_lossy().replace('\n', "").replace('\r', "");
                    if !p_str.is_empty() {
                        out.push(format!(
                            "fav_doc={}|{}|{}|{}",
                            id,
                            parent_id,
                            Self::escape_ini_field(title),
                            p_str
                        ));
                    }
                }
            }
        }
    }

    pub(crate) fn from_ini_entries(entries: &[&str]) -> Self {
        let mut tree = Self::default();
        let mut max_id = 0u64;
        for raw in entries {
            let line = raw.trim();
            if let Some(val) = line.strip_prefix("fav_folder=") {
                let parts = Self::split_escaped_pipes(val, 4);
                if parts.len() == 4 {
                    let id = parts[0].trim().parse::<u64>().unwrap_or(0);
                    let parent_id = parts[1].trim().parse::<u64>().unwrap_or(0);
                    let expanded = parts[2].trim() != "0";
                    let name = parts[3].clone();
                    if id > 0 {
                        max_id = max_id.max(id);
                        let node = FavNode::Folder {
                            id,
                            name,
                            expanded,
                            children: Vec::new(),
                        };
                        if parent_id == 0 || !Self::insert_into_folder(&mut tree.root, parent_id, node.clone()) {
                            tree.root.push(node);
                        }
                    }
                }
            } else if let Some(val) = line.strip_prefix("fav_doc=") {
                let parts = Self::split_escaped_pipes(val, 4);
                if parts.len() == 4 {
                    let id = parts[0].trim().parse::<u64>().unwrap_or(0);
                    let parent_id = parts[1].trim().parse::<u64>().unwrap_or(0);
                    let title = parts[2].clone();
                    let path = PathBuf::from(parts[3].trim());
                    if id > 0 && !path.as_os_str().is_empty() {
                        max_id = max_id.max(id);
                        let node = FavNode::Doc { id, title, path };
                        if parent_id == 0 || !Self::insert_into_folder(&mut tree.root, parent_id, node.clone()) {
                            tree.root.push(node);
                        }
                    }
                }
            }
        }
        tree.next_id = max_id + 1;
        tree
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_favorites_nested_folders_move_rename_delete_and_ini_roundtrip() {
        let mut fav = FavoritesTree::default();
        let work_id = fav.add_folder(None, "Work".to_string());
        let taxes_id = fav.add_folder(Some(work_id), "Taxes | 2026".to_string());
        let sub_id = fav.add_folder(Some(taxes_id), "Q1".to_string());

        let doc1 = fav.add_doc(Some(sub_id), "Return.pdf".to_string(), PathBuf::from("/docs/return.pdf"));
        let doc2 = fav.add_doc(None, "Notes.md".to_string(), PathBuf::from("/docs/notes.md"));

        assert!(fav.is_starred(Path::new("/docs/return.pdf")));
        assert!(!fav.is_starred(Path::new("/docs/other.pdf")));

        // Prevent moving a folder into its own descendant
        assert!(!fav.move_node(work_id, Some(sub_id)));
        // Move doc2 into Work -> Taxes
        assert!(fav.move_node(doc2, Some(taxes_id)));
        // Rename folder
        assert!(fav.rename_node(sub_id, "Quarter 1".to_string()));

        // INI round-trip preserves nested folders, escaped pipes, and doc paths
        let lines = fav.to_ini_lines();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let loaded = FavoritesTree::from_ini_entries(&refs);
        assert_eq!(loaded, fav);

        // Flatten & search filter
        let flat_all = loaded.flatten_visible("");
        assert_eq!(flat_all.len(), 5);
        let flat_q = loaded.flatten_visible("return");
        assert!(flat_q.iter().any(|r| r.id == doc1 && r.depth == 3));

        // Delete folder removes nested children
        let mut fav2 = loaded;
        assert!(fav2.delete_node(taxes_id));
        assert!(!fav2.is_starred(Path::new("/docs/return.pdf")));
        assert!(!fav2.is_starred(Path::new("/docs/notes.md")));
    }
}
