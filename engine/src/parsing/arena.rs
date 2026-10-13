//! Flat, id-indexed storage for expression trees.
//!
//! Nodes reference children by [`ExprId`], never by pointer. A table holds exactly one
//! tree: every child id is below its parent's id, and the root is the last node, so the
//! table order is a post-order of the tree. Trees of any depth are stored, walked,
//! compared and dropped without recursion.

use crate::parsing::source::Source;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExprId(u32);

impl ExprId {
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }

    #[must_use]
    pub fn from_index(index: usize) -> Self {
        Self(u32::try_from(index).expect("BUG: expression table exceeds u32::MAX nodes"))
    }
}

/// Node payload that names its child ids.
pub trait ExprKind: Clone + PartialEq {
    /// Calls `f` on each child id in left-to-right source order.
    fn for_each_child(&self, f: impl FnMut(ExprId));

    /// Mutable child ids, in the same order as [`ExprKind::for_each_child`].
    fn children_mut(&mut self) -> Vec<&mut ExprId>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExprNode<K> {
    pub kind: K,
    pub source_location: Option<Source>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExprTable<K> {
    nodes: Vec<ExprNode<K>>,
}

impl<K> Default for ExprTable<K> {
    fn default() -> Self {
        Self { nodes: Vec::new() }
    }
}

impl<K> ExprTable<K> {
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(capacity),
        }
    }

    /// Moves every node into a new table sized exactly; `self` keeps its capacity for reuse.
    pub fn take_compact(&mut self) -> Self {
        Self {
            nodes: self.nodes.drain(..).collect(),
        }
    }
}

impl<K: ExprKind> ExprTable<K> {
    /// Appends a node. Its children must already be in the table.
    pub fn push(&mut self, kind: K, source_location: Option<Source>) -> ExprId {
        let id = ExprId::from_index(self.nodes.len());
        kind.for_each_child(|child| {
            assert!(
                child < id,
                "BUG: child ExprId {} pushed after its parent {}",
                child.0,
                id.0
            );
        });
        self.nodes.push(ExprNode {
            kind,
            source_location,
        });
        id
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Drops every node pushed after the first `len`, undoing a speculative parse.
    pub fn truncate(&mut self, len: usize) {
        assert!(
            len <= self.nodes.len(),
            "BUG: truncate to {len} beyond table length {}",
            self.nodes.len()
        );
        self.nodes.truncate(len);
    }

    #[must_use]
    pub fn node(&self, id: ExprId) -> &ExprNode<K> {
        self.nodes.get(id.index()).unwrap_or_else(|| {
            panic!(
                "BUG: ExprId {} out of range (len {})",
                id.0,
                self.nodes.len()
            )
        })
    }

    #[must_use]
    pub fn kind(&self, id: ExprId) -> &K {
        &self.node(id).kind
    }

    #[must_use]
    pub fn source(&self, id: ExprId) -> Option<&Source> {
        self.node(id).source_location.as_ref()
    }

    /// Every node id, children before their parents.
    pub fn ids(&self) -> impl Iterator<Item = ExprId> {
        (0..self.nodes.len()).map(ExprId::from_index)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &ExprNode<K>> {
        self.nodes.iter()
    }

    pub fn kinds_mut(&mut self) -> impl Iterator<Item = &mut K> {
        self.nodes.iter_mut().map(|node| &mut node.kind)
    }

    pub fn kind_mut(&mut self, id: ExprId) -> &mut K {
        let len = self.nodes.len();
        &mut self
            .nodes
            .get_mut(id.index())
            .unwrap_or_else(|| panic!("BUG: ExprId {} out of range (len {len})", id.0))
            .kind
    }

    /// Structural equality of two subtrees, ignoring source locations and ids.
    #[must_use]
    pub fn structurally_equal(&self, left: ExprId, other: &Self, right: ExprId) -> bool {
        let mut lefts = vec![left];
        let mut rights = vec![right];
        while let (Some(a), Some(b)) = (lefts.pop(), rights.pop()) {
            let kind_a = self.kind(a);
            let kind_b = other.kind(b);
            if shape(kind_a) != shape(kind_b) {
                return false;
            }
            kind_a.for_each_child(|child| lefts.push(child));
            kind_b.for_each_child(|child| rights.push(child));
            if lefts.len() != rights.len() {
                return false;
            }
        }
        true
    }
}

fn shape<K: ExprKind>(kind: &K) -> K {
    let mut shaped = kind.clone();
    for child in shaped.children_mut() {
        *child = ExprId(0);
    }
    shaped
}

/// A table holding one tree, and that tree's root (its last node).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprTree<K> {
    pub table: ExprTable<K>,
    pub root: ExprId,
}

impl<K: ExprKind> ExprTree<K> {
    /// `table` as one tree rooted at its last node.
    #[must_use]
    pub fn new(table: ExprTable<K>, root: ExprId) -> Self {
        assert_eq!(
            root.index() + 1,
            table.len(),
            "BUG: expression tree root must be the last node of its table"
        );
        Self { table, root }
    }

    /// A tree of one node. `kind` must not name children.
    #[must_use]
    pub fn leaf(kind: K, source_location: Option<Source>) -> Self {
        let mut table = ExprTable::with_capacity(1);
        let root = table.push(kind, source_location);
        Self::new(table, root)
    }

    #[must_use]
    pub fn root_kind(&self) -> &K {
        self.table.kind(self.root)
    }

    #[must_use]
    pub fn root_source(&self) -> Option<&Source> {
        self.table.source(self.root)
    }

    #[must_use]
    pub fn at(&self, id: ExprId) -> ExprRef<'_, K> {
        ExprRef {
            table: &self.table,
            id,
        }
    }

    #[must_use]
    pub fn root(&self) -> ExprRef<'_, K> {
        self.at(self.root)
    }

    /// Every node is reachable from the root through exactly one parent.
    #[cfg(test)]
    pub(crate) fn assert_is_tree(&self) {
        let mut parents = vec![0usize; self.table.len()];
        for id in self.table.ids() {
            self.table
                .kind(id)
                .for_each_child(|child| parents[child.index()] += 1);
        }
        for (index, count) in parents.iter().enumerate() {
            let expected = usize::from(index != self.root.index());
            assert_eq!(
                *count, expected,
                "node {index} has {count} parents, expected {expected}"
            );
        }
    }
}

impl<K: ExprKind> PartialEq for ExprTree<K> {
    fn eq(&self, other: &Self) -> bool {
        self.table
            .structurally_equal(self.root, &other.table, other.root)
    }
}

impl<K: ExprKind> Eq for ExprTree<K> {}

/// A node viewed through its table.
#[derive(Debug)]
pub struct ExprRef<'a, K> {
    pub table: &'a ExprTable<K>,
    pub id: ExprId,
}

impl<K> Clone for ExprRef<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for ExprRef<'_, K> {}

impl<'a, K: ExprKind> ExprRef<'a, K> {
    #[must_use]
    pub fn kind(self) -> &'a K {
        self.table.kind(self.id)
    }

    #[must_use]
    pub fn source(self) -> Option<&'a Source> {
        self.table.source(self.id)
    }

    #[must_use]
    pub fn child(self, id: ExprId) -> Self {
        Self {
            table: self.table,
            id,
        }
    }

    /// Pushes this node's children onto a depth-first stack so they pop in source order.
    pub fn push_children(self, pending: &mut Vec<Self>) {
        let start = pending.len();
        self.kind()
            .for_each_child(|child| pending.push(self.child(child)));
        pending[start..].reverse();
    }
}

#[cfg(test)]
mod tests {
    use crate::parsing::source::SourceType;
    use crate::ResourceLimits;
    use std::path::{Path, PathBuf};

    const CORPUS_DIRS: [&str; 4] = [
        "documentation/examples",
        "benches/specs",
        "tests/fixtures",
        "../cli/tests/integrations",
    ];

    fn lemma_files(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).expect("corpus directory must be readable") {
                let path = entry.expect("corpus entry must be readable").path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|ext| ext == "lemma") {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }

    #[test]
    fn parsed_expressions_are_trees() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut checked = 0usize;
        for dir in CORPUS_DIRS {
            for path in lemma_files(&manifest.join(dir)) {
                let source = std::fs::read_to_string(&path).expect("corpus file must be readable");
                let specs = crate::parsing::parse(
                    &source,
                    SourceType::Volatile,
                    &ResourceLimits::default(),
                )
                .unwrap_or_else(|error| panic!("{} must parse: {error}", path.display()))
                .into_flattened_specs();
                for spec in &specs {
                    for rule in &spec.rules {
                        rule.expression.assert_is_tree();
                        for clause in &rule.unless_clauses {
                            clause.condition.assert_is_tree();
                            clause.result.assert_is_tree();
                        }
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 100, "corpus too small: {checked} rules");
    }
}
