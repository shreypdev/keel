//! The index of a derived list (ADR-039 section 3): two arena AVL order-statistic trees with parent
//! pointers.
//!
//! * The **positional** tree has one node per source row, in source order, and counts in every
//!   subtree the rows and the rows that pass the pipeline. The row's node id is its identity for
//!   life (everything else is keyed by it): `select(i)` finds the row at source index `i`, and the
//!   number of passing rows before it (its rank in an unsorted view) is a walk up the parents.
//! * The **sorted** tree (only for a view with a sort stage) has one node per passing row, ordered
//!   by `(sort key, source position)`, and counts the nodes of every subtree. A row's rank in the
//!   view is a walk up from its node, so a removal never searches: an `Ord` that is not a total
//!   order can misplace a row but cannot lose one.
//!
//! Both are the same [`OsTree`]: nodes in a `Vec` linked by `u32` ids and reused through a free
//! list, AVL-balanced (worst case O(log n) per operation, no randomness: R12), with a summable
//! aggregate in every subtree. Safe Rust, no dependency.

use std::cmp::Ordering;

/// The id no node has: an absent child, parent or node.
pub(crate) const NIL: u32 = u32::MAX;

/// What every subtree of an [`OsTree`] sums besides its size.
pub(crate) trait Aggregate: Copy + Default + PartialEq + std::fmt::Debug {
    /// One node's contribution, as the node stores it.
    type Own: Copy + Default + PartialEq + std::fmt::Debug;
    /// The aggregate of a single node.
    fn of(own: Self::Own) -> Self;
    /// The aggregate of two disjoint sets of nodes.
    fn add(self, other: Self) -> Self;
}

/// No aggregate beyond the size (the sorted tree).
impl Aggregate for () {
    type Own = ();
    fn of((): ()) {}
    fn add(self, (): ()) {}
}

/// How many rows of a subtree pass the pipeline (the positional tree).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pass(pub(crate) u32);

impl Aggregate for Pass {
    type Own = bool;
    fn of(own: bool) -> Pass {
        Pass(u32::from(own))
    }
    fn add(self, other: Pass) -> Pass {
        Pass(self.0 + other.0)
    }
}

/// One arena node. With `Pass` it is 24 bytes, the ADR's memory figure per source row.
#[derive(Clone, Copy, Debug)]
struct Node<A: Aggregate> {
    l: u32,
    r: u32,
    p: u32,
    /// Nodes in this subtree.
    size: u32,
    /// `own` summed over this subtree.
    sum: A,
    /// Height of this subtree (a leaf is 1).
    h: u8,
    own: A::Own,
}

impl<A: Aggregate> Node<A> {
    fn detached(own: A::Own) -> Node<A> {
        Node {
            l: NIL,
            r: NIL,
            p: NIL,
            size: 1,
            sum: A::of(own),
            h: 1,
            own,
        }
    }
}

/// An arena AVL tree ordered by position, with subtree sizes, a subtree aggregate and parent
/// pointers. Node ids are stable for the life of a node: [`detach`](OsTree::detach) unlinks a node
/// without freeing it, so a moved row keeps its id.
#[derive(Debug)]
pub(crate) struct OsTree<A: Aggregate> {
    nodes: Vec<Node<A>>,
    free: Vec<u32>,
    root: u32,
    len: usize,
}

impl<A: Aggregate> Default for OsTree<A> {
    fn default() -> Self {
        OsTree::new()
    }
}

impl<A: Aggregate> OsTree<A> {
    /// An empty tree.
    pub(crate) fn new() -> OsTree<A> {
        OsTree {
            nodes: Vec::new(),
            free: Vec::new(),
            root: NIL,
            len: 0,
        }
    }

    /// Nodes linked into the tree.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// The aggregate of every linked node. O(1).
    pub(crate) fn sum(&self) -> A {
        self.sum_of(self.root)
    }

    /// Allocates a detached node contributing `own`; reuses a freed id first.
    ///
    /// # Panics
    ///
    /// If the arena would hold `u32::MAX` nodes (a list that long cannot exist in memory).
    pub(crate) fn alloc(&mut self, own: A::Own) -> u32 {
        let node = Node::detached(own);
        if let Some(id) = self.free.pop() {
            self.nodes[id as usize] = node;
            return id;
        }
        let id = u32::try_from(self.nodes.len())
            .ok()
            .filter(|&id| id != NIL)
            .expect("undra-signals: a derived list cannot index more than u32::MAX - 1 rows");
        self.nodes.push(node);
        id
    }

    /// Returns a detached node's id to the free list.
    pub(crate) fn free(&mut self, id: u32) {
        debug_assert_eq!(self.nodes[id as usize].p, NIL, "freeing a linked node");
        self.free.push(id);
    }

    /// The number of ids the arena has handed out (live or free): the length a parallel `Vec`
    /// indexed by node id needs.
    pub(crate) fn capacity_ids(&self) -> usize {
        self.nodes.len()
    }

    /// Drops every node, keeping the arena's capacity. O(n) only for the aggregates' drop.
    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
        self.free.clear();
        self.root = NIL;
        self.len = 0;
    }

    /// This node's own contribution.
    pub(crate) fn own(&self, id: u32) -> A::Own {
        self.nodes[id as usize].own
    }

    /// Links detached node `id` so that it ends at in-order position `index` (`<= len`).
    pub(crate) fn attach_at(&mut self, id: u32, index: usize) {
        debug_assert!(index <= self.len, "attach_at {index} beyond {}", self.len);
        if self.root == NIL {
            self.link_root(id);
            return;
        }
        let mut x = self.root;
        let mut i = index;
        loop {
            let left = self.nodes[x as usize].l;
            let ls = self.size_of(left) as usize;
            if i <= ls {
                if left == NIL {
                    self.link_child(x, id, true);
                    break;
                }
                x = left;
            } else {
                i -= ls + 1;
                let right = self.nodes[x as usize].r;
                if right == NIL {
                    self.link_child(x, id, false);
                    break;
                }
                x = right;
            }
        }
        self.len += 1;
        self.rebalance_from(x);
    }

    /// Links detached node `id` by descending from the root: `goes_before(existing)` true means the
    /// new node belongs before `existing`; false (equal included) sends it after.
    pub(crate) fn attach_by(&mut self, id: u32, mut goes_before: impl FnMut(u32) -> bool) {
        if self.root == NIL {
            self.link_root(id);
            return;
        }
        let mut x = self.root;
        loop {
            let before = goes_before(x);
            let node = &self.nodes[x as usize];
            let next = if before { node.l } else { node.r };
            if next == NIL {
                self.link_child(x, id, before);
                break;
            }
            x = next;
        }
        self.len += 1;
        self.rebalance_from(x);
    }

    /// Unlinks `id` and rebalances. The node keeps its id and its `own` and can be attached again
    /// or freed.
    pub(crate) fn detach(&mut self, id: u32) {
        let Node { l, r, p, .. } = self.nodes[id as usize];
        debug_assert!(p != NIL || self.root == id, "detaching a detached node");
        let fix_from = if l != NIL && r != NIL {
            // The successor (leftmost of the right subtree) takes the node's place.
            let s = self.leftmost(r);
            let fix_from = if s == r {
                s
            } else {
                let sp = self.nodes[s as usize].p;
                let sr = self.nodes[s as usize].r;
                self.nodes[sp as usize].l = sr;
                if sr != NIL {
                    self.nodes[sr as usize].p = sp;
                }
                self.nodes[s as usize].r = r;
                self.nodes[r as usize].p = s;
                sp
            };
            self.nodes[s as usize].l = l;
            self.nodes[l as usize].p = s;
            self.nodes[s as usize].p = p;
            self.replace_child(p, id, s);
            fix_from
        } else {
            let child = if l != NIL { l } else { r };
            if child != NIL {
                self.nodes[child as usize].p = p;
            }
            self.replace_child(p, id, child);
            p
        };
        let own = self.nodes[id as usize].own;
        self.nodes[id as usize] = Node::detached(own);
        self.len -= 1;
        self.rebalance_from(fix_from);
    }

    /// The node at in-order position `index` (`< len`).
    pub(crate) fn select(&self, index: usize) -> u32 {
        debug_assert!(index < self.len, "select {index} of {}", self.len);
        let mut x = self.root;
        let mut i = index;
        loop {
            let node = &self.nodes[x as usize];
            let ls = self.size_of(node.l) as usize;
            match i.cmp(&ls) {
                Ordering::Less => x = node.l,
                Ordering::Equal => return x,
                Ordering::Greater => {
                    i -= ls + 1;
                    x = node.r;
                }
            }
        }
    }

    /// The number of nodes before `id` in order and their aggregate (a walk up the parents).
    pub(crate) fn before(&self, id: u32) -> (usize, A) {
        let node = &self.nodes[id as usize];
        let mut size = self.size_of(node.l) as usize;
        let mut sum = self.sum_of(node.l);
        let mut x = id;
        let mut p = node.p;
        while p != NIL {
            let parent = &self.nodes[p as usize];
            if parent.r == x {
                size += self.size_of(parent.l) as usize + 1;
                sum = sum.add(self.sum_of(parent.l)).add(A::of(parent.own));
            }
            x = p;
            p = parent.p;
        }
        (size, sum)
    }

    /// The in-order position of `id`.
    pub(crate) fn rank(&self, id: u32) -> usize {
        let node = &self.nodes[id as usize];
        let mut rank = self.size_of(node.l) as usize;
        let mut x = id;
        let mut p = node.p;
        while p != NIL {
            let parent = &self.nodes[p as usize];
            if parent.r == x {
                rank += self.size_of(parent.l) as usize + 1;
            }
            x = p;
            p = parent.p;
        }
        rank
    }

    /// Changes a linked node's contribution and re-sums its ancestors.
    pub(crate) fn set_own(&mut self, id: u32, own: A::Own) {
        self.nodes[id as usize].own = own;
        let mut x = id;
        while x != NIL {
            let Node { l, r, own, p, .. } = self.nodes[x as usize];
            self.nodes[x as usize].sum = self.sum_of(l).add(A::of(own)).add(self.sum_of(r));
            x = p;
        }
    }

    /// Links already allocated, detached nodes into an empty tree, in the given order, perfectly
    /// balanced (height `ceil(log2(n + 1))`). O(n).
    pub(crate) fn build(&mut self, ids_in_order: &[u32]) {
        debug_assert_eq!(self.root, NIL, "build needs an empty tree");
        self.root = self.build_range(ids_in_order, NIL);
        self.len = ids_in_order.len();
    }

    fn build_range(&mut self, ids: &[u32], parent: u32) -> u32 {
        if ids.is_empty() {
            return NIL;
        }
        // Recursion depth is the height of a perfectly balanced tree: log2(n).
        let mid = ids.len() / 2;
        let id = ids[mid];
        let l = self.build_range(&ids[..mid], id);
        let r = self.build_range(&ids[mid + 1..], id);
        let node = &mut self.nodes[id as usize];
        node.l = l;
        node.r = r;
        node.p = parent;
        self.pull(id);
        id
    }

    /// The first node in order, or [`NIL`].
    pub(crate) fn first(&self) -> u32 {
        if self.root == NIL {
            NIL
        } else {
            self.leftmost(self.root)
        }
    }

    /// The node after `id` in order, or [`NIL`]. Amortised O(1) over a full walk, which never
    /// recurses (a degenerate tree cannot overflow the stack).
    pub(crate) fn next(&self, id: u32) -> u32 {
        let node = &self.nodes[id as usize];
        if node.r != NIL {
            return self.leftmost(node.r);
        }
        let mut x = id;
        let mut p = node.p;
        while p != NIL && self.nodes[p as usize].r == x {
            x = p;
            p = self.nodes[p as usize].p;
        }
        p
    }

    // --- internals ---------------------------------------------------------------------------

    fn size_of(&self, id: u32) -> u32 {
        if id == NIL {
            0
        } else {
            self.nodes[id as usize].size
        }
    }

    fn sum_of(&self, id: u32) -> A {
        if id == NIL {
            A::default()
        } else {
            self.nodes[id as usize].sum
        }
    }

    fn height_of(&self, id: u32) -> u8 {
        if id == NIL {
            0
        } else {
            self.nodes[id as usize].h
        }
    }

    fn leftmost(&self, mut x: u32) -> u32 {
        while self.nodes[x as usize].l != NIL {
            x = self.nodes[x as usize].l;
        }
        x
    }

    fn link_root(&mut self, id: u32) {
        self.nodes[id as usize].p = NIL;
        self.root = id;
        self.len += 1;
    }

    fn link_child(&mut self, parent: u32, id: u32, left: bool) {
        if left {
            self.nodes[parent as usize].l = id;
        } else {
            self.nodes[parent as usize].r = id;
        }
        self.nodes[id as usize].p = parent;
    }

    /// Points `parent`'s link (or the root) that held `old` at `new`.
    fn replace_child(&mut self, parent: u32, old: u32, new: u32) {
        if parent == NIL {
            self.root = new;
        } else if self.nodes[parent as usize].l == old {
            self.nodes[parent as usize].l = new;
        } else {
            debug_assert_eq!(self.nodes[parent as usize].r, old);
            self.nodes[parent as usize].r = new;
        }
    }

    /// Recomputes a node's height, size and sum from its children.
    fn pull(&mut self, id: u32) {
        let Node { l, r, own, .. } = self.nodes[id as usize];
        let h = 1 + self.height_of(l).max(self.height_of(r));
        let size = self.size_of(l) + 1 + self.size_of(r);
        let sum = self.sum_of(l).add(A::of(own)).add(self.sum_of(r));
        let node = &mut self.nodes[id as usize];
        node.h = h;
        node.size = size;
        node.sum = sum;
    }

    /// Rotates `x`'s left child up; returns the subtree's new root.
    fn rotate_right(&mut self, x: u32) -> u32 {
        let y = self.nodes[x as usize].l;
        let p = self.nodes[x as usize].p;
        let yr = self.nodes[y as usize].r;
        self.nodes[x as usize].l = yr;
        if yr != NIL {
            self.nodes[yr as usize].p = x;
        }
        self.nodes[y as usize].r = x;
        self.nodes[x as usize].p = y;
        self.nodes[y as usize].p = p;
        self.replace_child(p, x, y);
        self.pull(x);
        self.pull(y);
        y
    }

    /// Rotates `x`'s right child up; returns the subtree's new root.
    fn rotate_left(&mut self, x: u32) -> u32 {
        let y = self.nodes[x as usize].r;
        let p = self.nodes[x as usize].p;
        let yl = self.nodes[y as usize].l;
        self.nodes[x as usize].r = yl;
        if yl != NIL {
            self.nodes[yl as usize].p = x;
        }
        self.nodes[y as usize].l = x;
        self.nodes[x as usize].p = y;
        self.nodes[y as usize].p = p;
        self.replace_child(p, x, y);
        self.pull(x);
        self.pull(y);
        y
    }

    /// Re-sums and rebalances every node from `x` up to the root.
    fn rebalance_from(&mut self, mut x: u32) {
        while x != NIL {
            self.pull(x);
            let Node { l, r, .. } = self.nodes[x as usize];
            let balance = i16::from(self.height_of(l)) - i16::from(self.height_of(r));
            if balance > 1 {
                let Node { l: ll, r: lr, .. } = self.nodes[l as usize];
                if self.height_of(ll) < self.height_of(lr) {
                    self.rotate_left(l);
                }
                x = self.rotate_right(x);
            } else if balance < -1 {
                let Node { l: rl, r: rr, .. } = self.nodes[r as usize];
                if self.height_of(rr) < self.height_of(rl) {
                    self.rotate_right(r);
                }
                x = self.rotate_left(x);
            }
            x = self.nodes[x as usize].p;
        }
    }

    /// The linked ids in order (tests and debug checks).
    #[cfg(test)]
    pub(crate) fn in_order(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.len);
        let mut x = self.first();
        while x != NIL {
            out.push(x);
            x = self.next(x);
        }
        out
    }

    /// Checks every structural invariant: parent links, heights, AVL balance, sizes, sums and the
    /// node count. Called by the tests after every operation.
    #[cfg(test)]
    pub(crate) fn validate(&self) {
        if self.root == NIL {
            assert_eq!(self.len, 0, "an empty tree has length 0");
            return;
        }
        assert_eq!(
            self.nodes[self.root as usize].p, NIL,
            "the root has no parent"
        );
        let (count, ..) = self.validate_at(self.root);
        assert_eq!(count as usize, self.len, "len matches the linked nodes");
    }

    #[cfg(test)]
    fn validate_at(&self, id: u32) -> (u32, u8, A) {
        let node = &self.nodes[id as usize];
        let (ls, lh, la) = if node.l == NIL {
            (0, 0, A::default())
        } else {
            assert_eq!(self.nodes[node.l as usize].p, id, "left child's parent");
            self.validate_at(node.l)
        };
        let (rs, rh, ra) = if node.r == NIL {
            (0, 0, A::default())
        } else {
            assert_eq!(self.nodes[node.r as usize].p, id, "right child's parent");
            self.validate_at(node.r)
        };
        let size = ls + 1 + rs;
        let h = 1 + lh.max(rh);
        let sum = la.add(A::of(node.own)).add(ra);
        assert_eq!(node.size, size, "size of {id}");
        assert_eq!(node.h, h, "height of {id}");
        assert_eq!(node.sum, sum, "sum of {id}");
        assert!(
            (i16::from(lh) - i16::from(rh)).abs() <= 1,
            "{id} is out of balance"
        );
        (size, h, sum)
    }

    /// The tree's height (tests).
    #[cfg(test)]
    pub(crate) fn height(&self) -> u8 {
        self.height_of(self.root)
    }
}

/// What one row did to the view when it was re-evaluated ([`DerivedIndex::transition`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// It was out of the view and still is.
    None,
    /// It entered the view at this rank.
    Insert(usize),
    /// It left the view from this rank.
    Remove(usize),
    /// It stays at this rank.
    Stay(usize),
    /// Its sort key changed and it moved from one rank to another.
    Move {
        /// Rank before.
        from: usize,
        /// Rank after (its final position, as SPEC 3.8 `Move` means it).
        to: usize,
    },
}

/// The index of one derived list: which source rows pass, in source order, and (with a sort
/// stage) the passing rows in view order. It never sees an item: the pipeline evaluates the rows
/// and hands it membership and sort keys (`Option<K>`, `K = ()` for an unsorted view).
#[derive(Debug)]
pub(crate) struct DerivedIndex<K> {
    sorted_view: bool,
    /// One node per source row, in source order; `own` = passes.
    pos: OsTree<Pass>,
    /// By positional id: the row's node in `srt`, or `NIL` (sorted views only).
    sorted_of: Vec<u32>,
    /// One node per passing row, by `(key, source position)` (sorted views only).
    srt: OsTree<()>,
    /// By sorted id: the positional id of its row.
    row_of: Vec<u32>,
    /// By sorted id: its sort key while the node is live.
    key_of: Vec<Option<K>>,
}

impl<K: Ord> DerivedIndex<K> {
    /// An empty index, with or without a sort stage.
    pub(crate) fn new(sorted_view: bool) -> DerivedIndex<K> {
        DerivedIndex {
            sorted_view,
            pos: OsTree::new(),
            sorted_of: Vec::new(),
            srt: OsTree::new(),
            row_of: Vec::new(),
            key_of: Vec::new(),
        }
    }

    /// Rows in the source.
    pub(crate) fn source_len(&self) -> usize {
        self.pos.len()
    }

    /// Rows in the view. O(1).
    pub(crate) fn view_len(&self) -> usize {
        self.pos.sum().0 as usize
    }

    /// The rank of a passing row in the view.
    fn rank_of(&self, row: u32) -> usize {
        if self.sorted_view {
            self.srt.rank(self.sorted_of[row as usize])
        } else {
            self.pos.before(row).1.0 as usize
        }
    }

    /// Allocates a positional node and keeps the parallel arrays as long as the arena.
    fn alloc_row(&mut self, passes: bool) -> u32 {
        let row = self.pos.alloc(passes);
        if self.sorted_view {
            if row as usize == self.sorted_of.len() {
                self.sorted_of.push(NIL);
            } else {
                self.sorted_of[row as usize] = NIL;
            }
        }
        row
    }

    /// Puts passing `row` into the sorted tree with `key`; returns its rank.
    fn attach_sorted(&mut self, row: u32, key: K) -> usize {
        let s = self.attach_sorted_quiet(row, key);
        self.srt.rank(s)
    }

    /// Puts passing `row` into the sorted tree with `key`; returns its sorted node.
    fn attach_sorted_quiet(&mut self, row: u32, key: K) -> u32 {
        let s = self.srt.alloc(());
        if s as usize == self.key_of.len() {
            self.key_of.push(Some(key));
            self.row_of.push(row);
        } else {
            self.key_of[s as usize] = Some(key);
            self.row_of[s as usize] = row;
        }
        self.sorted_of[row as usize] = s;
        self.place_sorted(s);
        s
    }

    /// Links detached sorted node `s` by its key and its row's current source position.
    fn place_sorted(&mut self, s: u32) {
        let row = self.row_of[s as usize];
        let at = self.pos.rank(row);
        let DerivedIndex {
            srt,
            key_of,
            row_of,
            pos,
            ..
        } = self;
        let key = key_of[s as usize]
            .as_ref()
            .expect("a live sorted node has its key");
        srt.attach_by(s, |y| {
            let other = key_of[y as usize]
                .as_ref()
                .expect("a live sorted node has its key");
            match key.cmp(other) {
                Ordering::Less => true,
                Ordering::Greater => false,
                // Ties keep source order, as a stable sort does (ADR-039 section 1).
                Ordering::Equal => at < pos.rank(row_of[y as usize]),
            }
        });
    }

    /// Takes passing `row` out of the sorted tree and frees its node.
    fn detach_sorted(&mut self, row: u32) {
        let s = self.sorted_of[row as usize];
        self.srt.detach(s);
        self.srt.free(s);
        self.key_of[s as usize] = None;
        self.sorted_of[row as usize] = NIL;
    }

    /// A row inserted at source index `at` (`<= source_len`), passing with `key` or not; its rank
    /// in the view when it passes. With `ranks == false` (nothing will be emitted: the index is
    /// all that changes) no rank is computed and a passing row reports rank 0.
    pub(crate) fn insert(&mut self, at: usize, key: Option<K>, ranks: bool) -> Option<usize> {
        let row = self.alloc_row(key.is_some());
        self.pos.attach_at(row, at);
        let key = key?;
        Some(match (self.sorted_view, ranks) {
            (true, true) => self.attach_sorted(row, key),
            (true, false) => {
                self.attach_sorted_quiet(row, key);
                0
            }
            (false, true) => self.rank_of(row),
            (false, false) => 0,
        })
    }

    /// The row at source index `at` (`< source_len`) removed; its rank when it was in the view
    /// (0 when `ranks == false`).
    pub(crate) fn remove(&mut self, at: usize, ranks: bool) -> Option<usize> {
        let row = self.pos.select(at);
        let passed = self.pos.own(row);
        let rank = (passed && ranks).then(|| self.rank_of(row));
        if passed && self.sorted_view {
            self.detach_sorted(row);
        }
        self.pos.detach(row);
        self.pos.free(row);
        passed.then(|| rank.unwrap_or(0))
    }

    /// The row at source index `at` re-evaluated to `now`.
    pub(crate) fn update(&mut self, at: usize, now: Option<K>, ranks: bool) -> Step {
        let row = self.pos.select(at);
        self.transition(row, now, ranks)
    }

    /// Whether evaluating `row` to `now` changes neither its membership nor its sort key: the
    /// parameter walk skips such rows before touching the trees.
    pub(crate) fn unchanged(&self, row: u32, now: &Option<K>) -> bool {
        let passes = self.pos.own(row);
        match now {
            None => !passes,
            Some(key) => {
                passes
                    && (!self.sorted_view
                        || self.key_of[self.sorted_of[row as usize] as usize].as_ref() == Some(key))
            }
        }
    }

    /// The positional id of the row at source index `at` (tests).
    #[cfg(test)]
    pub(crate) fn row_at(&self, at: usize) -> u32 {
        self.pos.select(at)
    }

    /// The rows in source order: first, then [`next_row`](DerivedIndex::next_row) until `NIL`.
    pub(crate) fn first_row(&self) -> u32 {
        self.pos.first()
    }

    /// The row after `row` in source order, or `NIL`.
    pub(crate) fn next_row(&self, row: u32) -> u32 {
        self.pos.next(row)
    }

    /// Whether `row` is in the view (tests).
    #[cfg(test)]
    pub(crate) fn passes(&self, row: u32) -> bool {
        self.pos.own(row)
    }

    /// Row `row` re-evaluated: its membership and sort key are now `now`. The shared transition
    /// of `Update` and of the parameter walk (ADR-039 section 2). With `ranks == false` the index
    /// changes exactly as it would otherwise, but no rank is computed: the step's ranks are 0, and
    /// a sort-key change is reported as `Move { from: 0, to: 0 }` whether or not the row moved.
    pub(crate) fn transition(&mut self, row: u32, now: Option<K>, ranks: bool) -> Step {
        let was = self.pos.own(row);
        let rank = |index: &Self| if ranks { index.rank_of(row) } else { 0 };
        match (was, now) {
            (false, None) => Step::None,
            (true, None) => {
                let rank = rank(self);
                if self.sorted_view {
                    self.detach_sorted(row);
                }
                self.pos.set_own(row, false);
                Step::Remove(rank)
            }
            (false, Some(key)) => {
                self.pos.set_own(row, true);
                if self.sorted_view {
                    self.attach_sorted_quiet(row, key);
                }
                Step::Insert(rank(self))
            }
            (true, Some(key)) => {
                if !self.sorted_view {
                    return Step::Stay(rank(self));
                }
                let s = self.sorted_of[row as usize];
                if self.key_of[s as usize].as_ref() == Some(&key) {
                    return Step::Stay(rank(self));
                }
                let from = rank(self);
                self.srt.detach(s);
                self.key_of[s as usize] = Some(key);
                self.place_sorted(s);
                if !ranks {
                    return Step::Move { from: 0, to: 0 };
                }
                let to = self.srt.rank(s);
                if from == to {
                    Step::Stay(to)
                } else {
                    Step::Move { from, to }
                }
            }
        }
    }

    /// The row at source index `from` moved so that it ends at `to` (SPEC 3.8 `Move`); its view
    /// ranks before and after when it is in the view and they differ (never computed, `None`,
    /// with `ranks == false`).
    pub(crate) fn move_row(
        &mut self,
        from: usize,
        to: usize,
        ranks: bool,
    ) -> Option<(usize, usize)> {
        let row = self.pos.select(from);
        let passes = self.pos.own(row);
        let before = (passes && ranks).then(|| self.rank_of(row));
        self.pos.detach(row);
        self.pos.attach_at(row, to);
        if passes && self.sorted_view {
            // Its key did not change; only its place among equal keys can.
            let s = self.sorted_of[row as usize];
            self.srt.detach(s);
            self.place_sorted(s);
        }
        let before = before?;
        let after = self.rank_of(row);
        (before != after).then_some((before, after))
    }

    /// Calls `f` with the source index of every row of the view, in view order, without
    /// allocating for an unsorted view (a sorted one needs one pass to number the rows). O(n).
    pub(crate) fn for_each_view_source(&self, mut f: impl FnMut(usize)) {
        if !self.sorted_view {
            let mut row = self.pos.first();
            let mut at = 0;
            while row != NIL {
                if self.pos.own(row) {
                    f(at);
                }
                at += 1;
                row = self.pos.next(row);
            }
            return;
        }
        let mut at_of = vec![0_usize; self.pos.capacity_ids()];
        let mut row = self.pos.first();
        let mut at = 0;
        while row != NIL {
            at_of[row as usize] = at;
            at += 1;
            row = self.pos.next(row);
        }
        let mut s = self.srt.first();
        while s != NIL {
            f(at_of[self.row_of[s as usize] as usize]);
            s = self.srt.next(s);
        }
    }

    /// Every row removed; whether the view had any.
    pub(crate) fn clear(&mut self) -> bool {
        let had = self.view_len() > 0;
        self.pos.clear();
        self.sorted_of.clear();
        self.srt.clear();
        self.row_of.clear();
        self.key_of.clear();
        had
    }

    /// Rebuilds the index from every row's membership and key, in source order: two balanced
    /// builds and a stable sort of the passing rows. O(n log n).
    ///
    /// # Panics
    ///
    /// Only if `K`'s `Ord` is not a total order and the standard library's sort detects it; the
    /// index is then left empty and the caller rebuilds again later.
    pub(crate) fn rebuild(&mut self, keys: Vec<Option<K>>) {
        self.clear();
        let mut rows = Vec::with_capacity(keys.len());
        let mut passing: Vec<(K, u32)> = Vec::new();
        for key in keys {
            let row = self.alloc_row(key.is_some());
            rows.push(row);
            if let Some(key) = key {
                if self.sorted_view {
                    passing.push((key, row));
                }
            }
        }
        self.pos.build(&rows);
        if !self.sorted_view {
            return;
        }
        // Stable: equal keys keep source order, the order `passing` was built in.
        passing.sort_by(|a, b| a.0.cmp(&b.0));
        let mut sorted = Vec::with_capacity(passing.len());
        for (key, row) in passing {
            let s = self.srt.alloc(());
            debug_assert_eq!(
                s as usize,
                self.key_of.len(),
                "a cleared arena allocates in order"
            );
            self.key_of.push(Some(key));
            self.row_of.push(row);
            self.sorted_of[row as usize] = s;
            sorted.push(s);
        }
        self.srt.build(&sorted);
    }

    /// The source index of every row of the view, in view order. O(n).
    pub(crate) fn view_sources(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.view_len());
        self.for_each_view_source(|at| out.push(at));
        out
    }

    /// Checks both trees and that they agree (tests).
    #[cfg(test)]
    pub(crate) fn validate(&self) {
        self.pos.validate();
        if !self.sorted_view {
            return;
        }
        self.srt.validate();
        assert_eq!(
            self.srt.len(),
            self.view_len(),
            "one sorted node per passing row"
        );
        let mut row = self.pos.first();
        while row != NIL {
            let s = self.sorted_of[row as usize];
            assert_eq!(self.pos.own(row), s != NIL, "membership of row {row}");
            if s != NIL {
                assert_eq!(self.row_of[s as usize], row, "back link of {s}");
                assert!(self.key_of[s as usize].is_some(), "key of {s}");
            }
            row = self.pos.next(row);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // --- the tree against a Vec model ---------------------------------------------------------

    /// A model of an `OsTree<Pass>`: ids in order with their `own`.
    #[derive(Default)]
    struct Model {
        order: Vec<(u32, bool)>,
    }

    impl Model {
        fn check(&self, tree: &OsTree<Pass>) {
            tree.validate();
            let ids: Vec<u32> = self.order.iter().map(|(id, _)| *id).collect();
            assert_eq!(tree.in_order(), ids, "in-order ids");
            assert_eq!(tree.len(), ids.len());
            let total = self.order.iter().filter(|(_, own)| *own).count();
            assert_eq!(tree.sum(), Pass(total as u32));
            for (i, (id, _)) in self.order.iter().enumerate() {
                assert_eq!(tree.select(i), *id, "select({i})");
                assert_eq!(tree.rank(*id), i, "rank({id})");
                let passing = self.order[..i].iter().filter(|(_, own)| *own).count();
                assert_eq!(tree.before(*id), (i, Pass(passing as u32)), "before({id})");
            }
        }
    }

    #[derive(Clone, Debug)]
    enum Op {
        Insert(usize, bool),
        Remove(usize),
        Move(usize, usize),
        SetOwn(usize, bool),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => (0_usize..200, any::<bool>()).prop_map(|(a, o)| Op::Insert(a, o)),
            2 => (0_usize..200).prop_map(Op::Remove),
            2 => (0_usize..200, 0_usize..200).prop_map(|(a, b)| Op::Move(a, b)),
            2 => (0_usize..200, any::<bool>()).prop_map(|(a, o)| Op::SetOwn(a, o)),
        ]
    }

    fn run(ops: &[Op]) {
        let mut tree = OsTree::<Pass>::new();
        let mut model = Model::default();
        let mut freed = Vec::new();
        for op in ops {
            let len = model.order.len();
            match *op {
                Op::Insert(at, own) => {
                    let at = at % (len + 1);
                    let id = tree.alloc(own);
                    if let Some(expected) = freed.pop() {
                        assert_eq!(id, expected, "a freed id is reused first");
                    }
                    tree.attach_at(id, at);
                    model.order.insert(at, (id, own));
                }
                Op::Remove(at) if len > 0 => {
                    let at = at % len;
                    let id = tree.select(at);
                    tree.detach(id);
                    tree.free(id);
                    freed.push(id);
                    model.order.remove(at);
                }
                Op::Move(a, b) if len > 0 => {
                    let (from, to) = (a % len, b % len);
                    let id = tree.select(from);
                    tree.detach(id);
                    tree.validate();
                    tree.attach_at(id, to);
                    let item = model.order.remove(from);
                    model.order.insert(to, item);
                }
                Op::SetOwn(at, own) if len > 0 => {
                    let at = at % len;
                    let id = tree.select(at);
                    tree.set_own(id, own);
                    model.order[at].1 = own;
                }
                _ => {}
            }
            model.check(&tree);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

        #[test]
        fn the_tree_agrees_with_a_vec_after_every_operation(ops in proptest::collection::vec(op(), 1..120)) {
            run(&ops);
        }

        #[test]
        fn attach_by_builds_the_order_of_a_stable_sort(keys in proptest::collection::vec(0_u8..6, 0..150)) {
            // Equal keys go after their equals, so inserting in source order is a stable sort.
            let mut tree = OsTree::<()>::new();
            for (i, _) in keys.iter().enumerate() {
                let id = tree.alloc(());
                assert_eq!(id as usize, i);
                let k = keys[i];
                tree.attach_by(id, |y| k < keys[y as usize]);
                tree.validate();
            }
            let mut expected: Vec<u32> = (0..keys.len() as u32).collect();
            expected.sort_by_key(|&i| keys[i as usize]);
            prop_assert_eq!(tree.in_order(), expected);
        }
    }

    #[test]
    fn a_fixed_sequence_through_every_rotation() {
        use Op::*;
        let mut ops = Vec::new();
        for i in 0..64 {
            ops.push(Insert(i, i % 3 == 0)); // ascending: right-heavy rotations
        }
        for _ in 0..32 {
            ops.push(Insert(0, true)); // descending: left-heavy rotations
        }
        for i in 0..40 {
            ops.push(Remove(i * 7));
            ops.push(Move(i * 5, i * 11));
            ops.push(SetOwn(i * 3, i % 2 == 0));
        }
        run(&ops);
    }

    #[test]
    fn build_is_perfectly_balanced() {
        for n in [0_usize, 1, 2, 3, 7, 8, 1000, 1023, 1024, 100_000] {
            let mut tree = OsTree::<Pass>::new();
            let ids: Vec<u32> = (0..n).map(|i| tree.alloc(i % 2 == 0)).collect();
            tree.build(&ids);
            tree.validate();
            let bound = usize::BITS - n.leading_zeros(); // ceil(log2(n + 1))
            assert!(
                u32::from(tree.height()) <= bound,
                "height {} for {n} nodes",
                tree.height()
            );
            assert_eq!(tree.sum(), Pass(n.div_ceil(2) as u32));
            if n > 0 {
                assert_eq!(tree.select(n - 1), ids[n - 1]);
            }
        }
    }

    #[test]
    fn a_walk_never_recurses_and_visits_in_order() {
        let mut tree = OsTree::<Pass>::new();
        for i in 0..10_000 {
            let id = tree.alloc(false);
            tree.attach_at(id, i);
        }
        let ids = tree.in_order();
        assert_eq!(ids, (0..10_000).collect::<Vec<u32>>());
        assert!(
            tree.height() <= 20,
            "AVL keeps 10,000 sequential inserts shallow"
        );
    }

    #[test]
    fn positional_nodes_are_24_bytes() {
        assert_eq!(std::mem::size_of::<Node<Pass>>(), 24);
        assert_eq!(std::mem::size_of::<Node<()>>(), 20);
    }

    #[test]
    fn heavy_ties_match_a_stable_sort() {
        // 10,000 rows, 4 keys: the comparison between equal keys reads source positions.
        let keys: Vec<u8> = (0..10_000_u32).map(|i| (i * 7 % 4) as u8).collect();
        let mut index = DerivedIndex::<u8>::new(true);
        for (i, &k) in keys.iter().enumerate() {
            index.insert(i, Some(k), true);
        }
        index.validate();
        let mut expected: Vec<usize> = (0..keys.len()).collect();
        expected.sort_by_key(|&i| keys[i]);
        assert_eq!(index.view_sources(), expected);
    }

    // --- the index against filter + stable sort ----------------------------------------------

    /// A source row in the model: membership and key (`None` = filtered out).
    type Row = Option<u8>;

    /// `filter + stable sort` of the model rows: the source indices of the view, in view order.
    fn reference(rows: &[Row], sorted: bool) -> Vec<usize> {
        let mut view: Vec<(u8, usize)> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.map(|k| (k, i)))
            .collect();
        if sorted {
            view.sort_by_key(|(k, _)| *k);
        }
        view.into_iter().map(|(_, i)| i).collect()
    }

    /// Applies the index's answer for one op to a host-side list of source rows.
    #[derive(Clone, Debug)]
    enum IOp {
        Insert(usize, Row),
        Remove(usize),
        Update(usize, Row),
        Move(usize, usize),
        Clear,
        Rebuild,
    }

    fn iop() -> impl Strategy<Value = IOp> {
        let row = prop_oneof![1 => Just(None), 3 => (0_u8..5).prop_map(Some)];
        prop_oneof![
            4 => (0_usize..64, row.clone()).prop_map(|(a, r)| IOp::Insert(a, r)),
            2 => (0_usize..64).prop_map(IOp::Remove),
            4 => (0_usize..64, row).prop_map(|(a, r)| IOp::Update(a, r)),
            2 => (0_usize..64, 0_usize..64).prop_map(|(a, b)| IOp::Move(a, b)),
            1 => Just(IOp::Clear),
            1 => Just(IOp::Rebuild),
        ]
    }

    /// The host list: identities of source rows (a counter), in view order.
    fn run_index(ops: &[IOp], sorted: bool) {
        let mut index = DerivedIndex::<u8>::new(sorted);
        // The same ops without ranks (what an unobserved list or a walk past its limit does): the
        // index must end up identical.
        let mut quiet = DerivedIndex::<u8>::new(sorted);
        let ranks = true;
        // The model: per source row, (identity, membership/key).
        let mut rows: Vec<(u32, Row)> = Vec::new();
        let mut host: Vec<u32> = Vec::new();
        let mut next = 0_u32;
        let rank_ok = |r: usize, len: usize| assert!(r <= len);
        for op in ops {
            let len = rows.len();
            match *op {
                IOp::Insert(at, row) => {
                    let at = at % (len + 1);
                    next += 1;
                    rows.insert(at, (next, row));
                    assert_eq!(quiet.insert(at, row, false).is_some(), row.is_some());
                    if let Some(r) = index.insert(at, row, ranks) {
                        rank_ok(r, host.len());
                        host.insert(r, next);
                    }
                }
                IOp::Remove(at) if len > 0 => {
                    let at = at % len;
                    rows.remove(at);
                    quiet.remove(at, false);
                    if let Some(r) = index.remove(at, ranks) {
                        host.remove(r);
                    }
                }
                IOp::Update(at, row) if len > 0 => {
                    let at = at % len;
                    rows[at].1 = row;
                    let row_id = quiet.row_at(at);
                    let unchanged = quiet.unchanged(row_id, &row);
                    let step = quiet.update(at, row, false);
                    assert_eq!(
                        unchanged,
                        matches!(step, Step::None | Step::Stay(_)),
                        "`unchanged` agrees with the transition"
                    );
                    match index.update(at, row, ranks) {
                        Step::None | Step::Stay(_) => {}
                        Step::Insert(r) => host.insert(r, rows[at].0),
                        Step::Remove(r) => {
                            host.remove(r);
                        }
                        Step::Move { from, to } => {
                            let id = host.remove(from);
                            host.insert(to, id);
                        }
                    }
                }
                IOp::Move(a, b) if len > 0 => {
                    let (from, to) = (a % len, b % len);
                    let row = rows.remove(from);
                    rows.insert(to, row);
                    assert_eq!(quiet.move_row(from, to, false), None);
                    if let Some((f, t)) = index.move_row(from, to, ranks) {
                        let id = host.remove(f);
                        host.insert(t, id);
                    }
                }
                IOp::Clear => {
                    rows.clear();
                    index.clear();
                    quiet.clear();
                    host.clear();
                }
                IOp::Rebuild => {
                    index.rebuild(rows.iter().map(|(_, r)| *r).collect());
                    quiet.rebuild(rows.iter().map(|(_, r)| *r).collect());
                    host = reference(&rows.iter().map(|(_, r)| *r).collect::<Vec<_>>(), sorted)
                        .into_iter()
                        .map(|i| rows[i].0)
                        .collect();
                }
                _ => {}
            }
            index.validate();
            quiet.validate();
            assert_eq!(
                quiet.view_sources(),
                index.view_sources(),
                "the quiet twin after {op:?}"
            );
            let model: Vec<Row> = rows.iter().map(|(_, r)| *r).collect();
            let expected = reference(&model, sorted);
            assert_eq!(index.view_sources(), expected, "after {op:?}");
            assert_eq!(index.view_len(), expected.len());
            let expected_ids: Vec<u32> = expected.iter().map(|&i| rows[i].0).collect();
            assert_eq!(host, expected_ids, "host after {op:?}");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

        #[test]
        fn an_unsorted_index_reports_ranks_that_keep_a_host_in_step(ops in proptest::collection::vec(iop(), 1..100)) {
            run_index(&ops, false);
        }

        #[test]
        fn a_sorted_index_with_heavy_ties_keeps_a_host_in_step(ops in proptest::collection::vec(iop(), 1..100)) {
            run_index(&ops, true);
        }
    }

    #[test]
    fn an_inconsistent_order_never_loses_or_duplicates_a_row() {
        /// A key whose comparison is a deterministic pseudo-random answer, not a total order.
        #[derive(Clone, Debug)]
        struct Chaos(u32);
        impl PartialEq for Chaos {
            fn eq(&self, other: &Self) -> bool {
                self.cmp(other) == Ordering::Equal
            }
        }
        impl Eq for Chaos {}
        impl PartialOrd for Chaos {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }
        impl Ord for Chaos {
            fn cmp(&self, other: &Self) -> Ordering {
                let mix = self.0.wrapping_mul(2_654_435_761) ^ other.0.wrapping_mul(40_503);
                match mix % 3 {
                    0 => Ordering::Less,
                    1 => Ordering::Equal,
                    _ => Ordering::Greater,
                }
            }
        }
        let mut index = DerivedIndex::<Chaos>::new(true);
        let mut len = 0_usize;
        let mut passing = 0_usize;
        for i in 0..3_000_u32 {
            let at = (i as usize * 31) % (len + 1);
            let pass = i % 5 != 0;
            index.insert(at, pass.then_some(Chaos(i)), true);
            len += 1;
            passing += usize::from(pass);
            if i % 3 == 0 && len > 1 {
                let at = (i as usize * 17) % len;
                let key = (i % 2 == 0).then_some(Chaos(i ^ 0xFF));
                let was = index.passes(index.row_at(at));
                index.update(at, key.clone(), true);
                passing = passing - usize::from(was) + usize::from(key.is_some());
            }
            if i % 7 == 0 && len > 1 {
                index.move_row((i as usize * 13) % len, (i as usize * 3) % len, true);
            }
            if i % 11 == 0 && len > 1 {
                let at = (i as usize * 19) % len;
                let was = index.passes(index.row_at(at));
                index.remove(at, true);
                len -= 1;
                passing -= usize::from(was);
            }
        }
        index.validate();
        assert_eq!(index.source_len(), len);
        assert_eq!(index.view_len(), passing);
        let mut seen = index.view_sources();
        assert_eq!(seen.len(), passing);
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), passing, "no row is duplicated or lost");
    }
}
