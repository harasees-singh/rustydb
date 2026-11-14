use crate::cache::policy::{EntryId, EvictionPolicy};

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
    fn sentinel() -> Self {
        Node {
            prev: u32::MAX,
            next: u32::MAX,
            charge: 0,
            in_list: false,
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
        let mut nodes: Vec<Node> = (0..2).map(|_| Node::sentinel()).collect();
        nodes[0].prev = u32::MAX; // head
        nodes[1].next = u32::MAX; // tail
        nodes[0].next = 1;
        nodes[1].prev = 0;

        LRUEvictionPolicy {
            nodes,
            head: 0,
            tail: 1,
            charge: 0,
            capacity,
        }
    }

    fn insert(&mut self, id: EntryId, hash: u64, charge: usize, evicted: &mut Vec<EntryId>) {
        todo!()
    }

    fn access(&mut self, id: EntryId, hash: u64) {
        todo!()
    }

    fn remove(&mut self, id: EntryId) {
        todo!()
    }

    fn clear(&mut self) {
        todo!()
    }

    fn set_capacity(&mut self, capacity: usize, evicted: &mut Vec<EntryId>) {
        todo!()
    }

    fn charge(&self) -> usize {
        todo!()
    }

    fn capacity(&self) -> usize {
        todo!()
    }
}

impl LRUEvictionPolicy {
    fn promote(&mut self, id: EntryId, hash: u64) {
        let p = self.nodes[id.index()].prev;
        let n = self.nodes[id.index()].next;

        // p's next should point to n and n's prev should point to p
        // the node is promoted so node's next becomes head, head's prev becomes node
        // node becomes the head

        let p_node = self.nodes.get_mut(p as usize);
        if let Some(p_node_) = p_node {
            p_node_.next = n;
        }

        let n_node = self.nodes.get_mut(n as usize);
        if let Some(n_node_) = n_node {
            n_node_.prev = p;
        }

        let head = self.nodes.get_mut(self.head as usize).map(|n| n.next); // node next to sentinel
        if let Some(head_) = head {
            if let Some(head_node_) = self.nodes.get_mut(head_ as usize) {
                head_node_.prev = id.index() as u32;
                if let Some(cur_node_) = self.nodes.get_mut(id.index()) {
                    cur_node_.next = head_;
                    cur_node_.prev = self.head;
                }
            }
        }
        self.nodes[self.head as usize].next = id.index() as u32;
    }
}
