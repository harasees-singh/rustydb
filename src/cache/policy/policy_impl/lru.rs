use crate::cache::policy::{EntryId, EvictionPolicy};

/// "No neighbour": marks the ends of the list and an empty list.
const NIL: u32 = u32::MAX;

/// Least-recently-used eviction.
#[derive(Clone)]
struct Node {
    prev: u32,
    next: u32,
    charge: usize,
    in_list: bool,
}

impl Node {
    fn new(prev: u32, next: u32, charge: usize, in_list: bool) -> Self {
        Self {
            prev,
            next,
            charge,
            in_list,
        }
    }
}
pub struct LRUEvictionPolicy {
    nodes: Vec<Node>, // grown on demand to cover any EntryId.index()
    head: u32,        // most recently used (or a NIL marker when empty)
    tail: u32,        // least recently used, the eviction end
    charge: usize,    // sum of tracked charges
    capacity: usize,
}

impl EvictionPolicy for LRUEvictionPolicy {
    fn with_capacity(capacity: usize) -> Self
    where
        Self: Sized,
    {
        LRUEvictionPolicy {
            nodes: Vec::new(),
            head: NIL,
            tail: NIL,
            charge: 0,
            capacity,
        }
    }

    fn insert(&mut self, id: EntryId, _hash: u64, charge: usize, evicted: &mut Vec<EntryId>) -> bool {
        // Can never fit: reject it without touching any state.
        if charge > self.capacity {
            evicted.push(id);
            return false;
        }

        // Make room first, so the total never goes over capacity.
        let limit = self.capacity - charge;
        self.evict_until(limit, evicted);

        // Track the new entry as the most recently used.
        let i = id.index();
        if i >= self.nodes.len() {
            // Slots below `i` that were never used (their ids were rejected)
            // are vacant.
            self.nodes.resize_with(i + 1, || Node::new(NIL, NIL, 0, false));
        }
        debug_assert!(!self.nodes[i].in_list, "id inserted twice");
        self.nodes[i] = Node::new(NIL, NIL, charge, true);
        self.push_front(i as u32);
        self.charge += charge;

        true
    }

    fn access(&mut self, id: EntryId, _hash: u64) {
        self.promote(id);
    }

    fn remove(&mut self, id: EntryId) {
        let tracked = self.nodes.get(id.index()).is_some_and(|n| n.in_list);
        debug_assert!(tracked, "removing an untracked id");
        if tracked {
            self.untrack(id.index() as u32);
        }
    }

    fn clear(&mut self) {
        self.nodes = Vec::new();
        self.head = NIL;
        self.tail = NIL;
        self.charge = 0;
    }

    fn set_capacity(&mut self, capacity: usize, evicted: &mut Vec<EntryId>) {
        self.capacity = capacity;
        self.evict_until(capacity, evicted);
    }

    fn charge(&self) -> usize {
        self.charge
    }

    fn capacity(&self) -> usize {
        self.capacity
    }
}

impl LRUEvictionPolicy {
    /// Stops tracking node `i`: detaches it, subtracts its charge and marks
    /// the slot vacant.
    fn untrack(&mut self, i: u32) {
        self.unlink(i);
        let node = &mut self.nodes[i as usize];
        self.charge -= node.charge;
        node.charge = 0;
        node.in_list = false;
    }

    /// Evicts from the least-recently-used end, appending each victim to
    /// `evicted`, until the total charge is at most `limit`.
    fn evict_until(&mut self, limit: usize, evicted: &mut Vec<EntryId>) {
        while self.charge > limit {
            let victim = self.tail;
            debug_assert!(victim != NIL, "over the limit but the list is empty");
            if victim == NIL {
                break;
            }
            self.untrack(victim);
            evicted.push(EntryId::new(victim));
        }
    }

    /// Detaches node `i` from the list, fixing its neighbours and, if it was
    /// the first or last node, `head` or `tail`, and marks it not linked.
    /// Does not touch `charge`: the callers own that.
    fn unlink(&mut self, i: u32) {
        let (p, n) = (self.nodes[i as usize].prev, self.nodes[i as usize].next);
        if p != NIL {
            self.nodes[p as usize].next = n;
        } else {
            self.head = n; // `i` was the head
        }
        if n != NIL {
            self.nodes[n as usize].prev = p;
        } else {
            self.tail = p; // `i` was the tail
        }
        self.nodes[i as usize].prev = NIL;
        self.nodes[i as usize].next = NIL;
        self.nodes[i as usize].in_list = false;
    }

    /// Links the detached node `i` at the head, as the most recently used.
    fn push_front(&mut self, i: u32) {
        let old_head = self.head;
        self.nodes[i as usize].prev = NIL;
        self.nodes[i as usize].next = old_head;
        self.nodes[i as usize].in_list = true; // `in_list` means "currently linked"
        if old_head != NIL {
            self.nodes[old_head as usize].prev = i;
        } else {
            self.tail = i; // the list was empty, so `i` is also the tail
        }
        self.head = i;
    }

    /// Moves a tracked entry to the most-recently-used end.
    fn promote(&mut self, id: EntryId) {
        let i = id.index() as u32;
        debug_assert!(self.nodes[id.index()].in_list, "promoting an untracked id");
        if self.head == i {
            return; // already the most recently used
        }
        self.unlink(i);
        self.push_front(i);
    }
}
