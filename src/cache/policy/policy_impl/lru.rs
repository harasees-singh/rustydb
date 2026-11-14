use crate::cache::policy::{EntryId, EvictionPolicy};

/// "No neighbour": marks the ends of the list and an empty list.
const NIL: u32 = u32::MAX;

/// Least-recently-used eviction.
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

    fn insert(&mut self, id: EntryId, hash: u64, charge: usize, evicted: &mut Vec<EntryId>) -> bool {
        if charge > self.capacity {
            return false;
        }
        while self.charge + charge > self.capacity {
            let entry = EntryId::new(self.tail);
            self.remove(entry);
            evicted.push(entry);
        }
        let node = Node::new(NIL, self.head, charge, true);
        if id.index() >= self.nodes.len() {
            self.nodes.push(node)
        }
        else {
            self.nodes[id.index()] = node;
        }
        self.promote(id);
        self.capacity += charge;

        true
    }

    fn access(&mut self, id: EntryId, _hash: u64) {
        self.promote(id);
    }

    fn remove(&mut self, id: EntryId) {
        self.charge -= self.nodes[id.index()].charge;
        self.unlink(id.index() as u32);
    }

    fn clear(&mut self) {
        self.nodes = Vec::new();
        self.head = NIL;
        self.tail = NIL;
        self.charge = 0;
    }

    fn set_capacity(&mut self, capacity: usize, evicted: &mut Vec<EntryId>) {
        while self.capacity > capacity {
            let entry = EntryId::new(self.tail);
            self.remove(entry);
            evicted.push(entry);
        }
        self.capacity = capacity;
    }

    fn charge(&self) -> usize {
        self.charge
    }

    fn capacity(&self) -> usize {
        self.capacity
    }
}

impl LRUEvictionPolicy {
    /// Detaches node `i` from the list, fixing its neighbours and, if it was
    /// the first or last node, `head` or `tail`. Does not touch `charge` or
    /// `in_list`: the callers own those.
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
    }

    /// Links the detached node `i` at the head, as the most recently used.
    fn push_front(&mut self, i: u32) {
        let old_head = self.head;
        self.nodes[i as usize].prev = NIL;
        self.nodes[i as usize].next = old_head;
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
