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

    fn insert(
        &mut self,
        id: EntryId,
        _hash: u64,
        charge: usize,
        evicted: &mut Vec<EntryId>,
    ) -> bool {
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
            self.nodes
                .resize_with(i + 1, || Node::new(NIL, NIL, 0, false));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn id(n: u32) -> EntryId {
        EntryId::new(n)
    }

    /// Inserts `n` and returns whether it was admitted and which ids were evicted.
    fn put(p: &mut LRUEvictionPolicy, n: u32, charge: usize) -> (bool, Vec<usize>) {
        let mut evicted = Vec::new();
        let admitted = p.insert(id(n), 0, charge, &mut evicted);
        check(p);
        (admitted, evicted.iter().map(|e| e.index()).collect())
    }

    fn set_capacity(p: &mut LRUEvictionPolicy, capacity: usize) -> Vec<usize> {
        let mut evicted = Vec::new();
        p.set_capacity(capacity, &mut evicted);
        check(p);
        evicted.iter().map(|e| e.index()).collect()
    }

    fn touch(p: &mut LRUEvictionPolicy, n: u32) {
        p.access(id(n), 0);
        check(p);
    }

    fn remove(p: &mut LRUEvictionPolicy, n: u32) {
        p.remove(id(n));
        check(p);
    }

    /// Tracked ids from most to least recently used.
    fn order(p: &LRUEvictionPolicy) -> Vec<usize> {
        let mut out = Vec::new();
        let mut cur = p.head;
        while cur != NIL {
            out.push(cur as usize);
            assert!(out.len() <= p.nodes.len(), "cycle in the list");
            cur = p.nodes[cur as usize].next;
        }
        out
    }

    /// Checks every structural invariant of the policy.
    fn check(p: &LRUEvictionPolicy) {
        let forward = order(p);

        let mut backward = Vec::new();
        let mut cur = p.tail;
        while cur != NIL {
            backward.push(cur as usize);
            assert!(backward.len() <= p.nodes.len(), "cycle in the list");
            cur = p.nodes[cur as usize].prev;
        }
        backward.reverse();
        assert_eq!(forward, backward, "forward and backward walks disagree");

        let mut sorted = forward.clone();
        sorted.sort();
        let linked: Vec<usize> = (0..p.nodes.len()).filter(|&i| p.nodes[i].in_list).collect();
        assert_eq!(sorted, linked, "in_list flags and the list disagree");

        for (i, node) in p.nodes.iter().enumerate() {
            if !node.in_list {
                assert_eq!(
                    (node.prev, node.next, node.charge),
                    (NIL, NIL, 0),
                    "vacant node {i} is dirty"
                );
            }
        }

        let sum: usize = forward.iter().map(|&i| p.nodes[i].charge).sum();
        assert_eq!(p.charge, sum, "charge is out of step with the nodes");
        assert!(p.charge <= p.capacity, "over capacity");
        if forward.is_empty() {
            assert_eq!((p.head, p.tail), (NIL, NIL));
        }
    }

    #[test]
    fn starts_empty() {
        let p = LRUEvictionPolicy::with_capacity(10);
        assert_eq!(p.charge(), 0);
        assert_eq!(p.capacity(), 10);
        assert!(order(&p).is_empty());
        check(&p);
    }

    #[test]
    fn inserts_most_recent_first_and_tracks_charge() {
        let mut p = LRUEvictionPolicy::with_capacity(10);
        assert_eq!(put(&mut p, 0, 2), (true, vec![]));
        assert_eq!(put(&mut p, 1, 3), (true, vec![]));
        assert_eq!(put(&mut p, 2, 1), (true, vec![]));
        assert_eq!(order(&p), [2, 1, 0]);
        assert_eq!(p.charge(), 6);
    }

    #[test]
    fn evicts_from_the_least_recently_used_end() {
        let mut p = LRUEvictionPolicy::with_capacity(3);
        for n in 0..3 {
            put(&mut p, n, 1);
        }
        assert_eq!(put(&mut p, 3, 1), (true, vec![0]));
        assert_eq!(put(&mut p, 4, 1), (true, vec![1]));
        assert_eq!(order(&p), [4, 3, 2]);
        assert_eq!(p.charge(), 3);
    }

    #[test]
    fn one_large_insert_can_evict_several() {
        let mut p = LRUEvictionPolicy::with_capacity(5);
        for n in 0..5 {
            put(&mut p, n, 1);
        }
        assert_eq!(put(&mut p, 9, 3), (true, vec![0, 1, 2]));
        assert_eq!(order(&p), [9, 4, 3]);
        assert_eq!(p.charge(), 5);
    }

    #[test]
    fn access_promotes_so_the_entry_is_evicted_last() {
        let mut p = LRUEvictionPolicy::with_capacity(3);
        for n in 0..3 {
            put(&mut p, n, 1);
        }
        touch(&mut p, 0);
        assert_eq!(order(&p), [0, 2, 1]);
        assert_eq!(put(&mut p, 3, 1), (true, vec![1]));
        assert_eq!(order(&p), [3, 0, 2]);
    }

    #[test]
    fn access_works_at_the_head_tail_and_middle() {
        let mut p = LRUEvictionPolicy::with_capacity(10);
        for n in 0..4 {
            put(&mut p, n, 1);
        }
        assert_eq!(order(&p), [3, 2, 1, 0]);
        touch(&mut p, 3); // head: nothing changes
        assert_eq!(order(&p), [3, 2, 1, 0]);
        touch(&mut p, 0); // tail
        assert_eq!(order(&p), [0, 3, 2, 1]);
        touch(&mut p, 2); // middle
        assert_eq!(order(&p), [2, 0, 3, 1]);
        assert_eq!(p.charge(), 4);
    }

    #[test]
    fn access_on_a_single_entry_is_harmless() {
        let mut p = LRUEvictionPolicy::with_capacity(3);
        put(&mut p, 0, 1);
        touch(&mut p, 0);
        assert_eq!(order(&p), [0]);
    }

    #[test]
    fn rejects_an_entry_bigger_than_the_capacity_without_changing_anything() {
        let mut p = LRUEvictionPolicy::with_capacity(5);
        put(&mut p, 0, 2);
        put(&mut p, 1, 2);
        assert_eq!(put(&mut p, 2, 6), (false, vec![2]));
        assert_eq!(order(&p), [1, 0]);
        assert_eq!(p.charge(), 4);
    }

    #[test]
    fn rejected_ids_leave_no_trace_and_can_be_used_later() {
        let mut p = LRUEvictionPolicy::with_capacity(5);
        // id 4 is rejected before the nodes vec has grown to cover it.
        assert_eq!(put(&mut p, 4, 9), (false, vec![4]));
        assert_eq!(put(&mut p, 1, 1), (true, vec![]));
        assert_eq!(put(&mut p, 4, 1), (true, vec![]));
        assert_eq!(order(&p), [4, 1]);
    }

    #[test]
    fn an_entry_exactly_the_size_of_the_capacity_is_admitted() {
        let mut p = LRUEvictionPolicy::with_capacity(4);
        put(&mut p, 0, 1);
        put(&mut p, 1, 1);
        assert_eq!(put(&mut p, 2, 4), (true, vec![0, 1]));
        assert_eq!(order(&p), [2]);
        assert_eq!(p.charge(), 4);
    }

    #[test]
    fn zero_capacity_rejects_everything_that_costs_something() {
        let mut p = LRUEvictionPolicy::with_capacity(0);
        assert_eq!(put(&mut p, 0, 1), (false, vec![0]));
        assert_eq!(p.charge(), 0);
        // A free entry still fits.
        assert_eq!(put(&mut p, 1, 0), (true, vec![]));
        assert_eq!(order(&p), [1]);
    }

    #[test]
    fn remove_works_at_the_head_tail_middle_and_when_alone() {
        let mut p = LRUEvictionPolicy::with_capacity(20);
        for n in 0..5 {
            put(&mut p, n, n as usize + 1);
        }
        assert_eq!(order(&p), [4, 3, 2, 1, 0]);
        remove(&mut p, 2); // middle
        assert_eq!(order(&p), [4, 3, 1, 0]);
        remove(&mut p, 4); // head
        assert_eq!(order(&p), [3, 1, 0]);
        remove(&mut p, 0); // tail
        assert_eq!(order(&p), [3, 1]);
        assert_eq!(p.charge(), 4 + 2);
        remove(&mut p, 1);
        remove(&mut p, 3); // alone
        assert!(order(&p).is_empty());
        assert_eq!(p.charge(), 0);
    }

    #[test]
    fn remove_frees_room_so_nothing_is_evicted() {
        let mut p = LRUEvictionPolicy::with_capacity(2);
        put(&mut p, 0, 1);
        put(&mut p, 1, 1);
        remove(&mut p, 0);
        assert_eq!(put(&mut p, 2, 1), (true, vec![]));
        assert_eq!(order(&p), [2, 1]);
    }

    #[test]
    fn a_removed_id_can_be_inserted_again() {
        let mut p = LRUEvictionPolicy::with_capacity(5);
        put(&mut p, 0, 2);
        put(&mut p, 1, 2);
        remove(&mut p, 0);
        assert_eq!(put(&mut p, 0, 4), (true, vec![1]));
        assert_eq!(order(&p), [0]);
        assert_eq!(p.charge(), 4);
    }

    #[test]
    fn ids_need_not_arrive_in_order() {
        let mut p = LRUEvictionPolicy::with_capacity(10);
        put(&mut p, 7, 1);
        put(&mut p, 2, 1);
        put(&mut p, 5, 1);
        assert_eq!(order(&p), [5, 2, 7]);
        touch(&mut p, 7);
        remove(&mut p, 2);
        assert_eq!(order(&p), [7, 5]);
    }

    #[test]
    fn shrinking_the_capacity_evicts_the_least_recently_used() {
        let mut p = LRUEvictionPolicy::with_capacity(5);
        for n in 0..5 {
            put(&mut p, n, 1);
        }
        touch(&mut p, 0);
        assert_eq!(set_capacity(&mut p, 2), [1, 2, 3]);
        assert_eq!(order(&p), [0, 4]);
        assert_eq!(p.capacity(), 2);
        assert_eq!(p.charge(), 2);
    }

    #[test]
    fn growing_the_capacity_evicts_nothing_and_makes_room() {
        let mut p = LRUEvictionPolicy::with_capacity(2);
        put(&mut p, 0, 1);
        put(&mut p, 1, 1);
        assert_eq!(set_capacity(&mut p, 4), Vec::<usize>::new());
        assert_eq!(put(&mut p, 2, 1), (true, vec![]));
        assert_eq!(put(&mut p, 3, 1), (true, vec![]));
        assert_eq!(order(&p), [3, 2, 1, 0]);
    }

    #[test]
    fn capacity_zero_evicts_everything() {
        let mut p = LRUEvictionPolicy::with_capacity(3);
        for n in 0..3 {
            put(&mut p, n, 1);
        }
        assert_eq!(set_capacity(&mut p, 0), [0, 1, 2]);
        assert!(order(&p).is_empty());
        assert_eq!(p.charge(), 0);
    }

    #[test]
    fn clear_forgets_everything_and_the_policy_is_reusable() {
        let mut p = LRUEvictionPolicy::with_capacity(4);
        for n in 0..4 {
            put(&mut p, n, 1);
        }
        p.clear();
        check(&p);
        assert_eq!(p.charge(), 0);
        assert_eq!(p.capacity(), 4, "clear keeps the capacity");
        assert!(order(&p).is_empty());
        assert_eq!(put(&mut p, 0, 1), (true, vec![]));
        assert_eq!(order(&p), [0]);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "id inserted twice")]
    fn inserting_a_tracked_id_again_is_caught_in_debug_builds() {
        let mut p = LRUEvictionPolicy::with_capacity(5);
        put(&mut p, 0, 1);
        put(&mut p, 0, 1);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "removing an untracked id")]
    fn removing_an_untracked_id_is_caught_in_debug_builds() {
        let mut p = LRUEvictionPolicy::with_capacity(5);
        p.remove(id(3));
    }

    #[test]
    fn matches_a_simple_model_under_random_operations() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        let mut capacity = 10usize;
        let mut p = LRUEvictionPolicy::with_capacity(capacity);
        // (id, charge), front = most recently used.
        let mut model: VecDeque<(u32, usize)> = VecDeque::new();
        let total = |m: &VecDeque<(u32, usize)>| m.iter().map(|e| e.1).sum::<usize>();

        for step in 0..50_000 {
            match next() % 10 {
                // insert a fresh id
                0..=4 => {
                    let n = (next() % 24) as u32;
                    if model.iter().any(|e| e.0 == n) {
                        continue;
                    }
                    let charge = (next() % 5) as usize;
                    let (admitted, evicted) = put(&mut p, n, charge);

                    let mut expected = Vec::new();
                    if charge > capacity {
                        expected.push(n as usize);
                    } else {
                        let mut sum = total(&model);
                        while sum + charge > capacity {
                            let victim = model.pop_back().unwrap();
                            sum -= victim.1;
                            expected.push(victim.0 as usize);
                        }
                        model.push_front((n, charge));
                    }
                    assert_eq!(admitted, charge <= capacity, "step {step}");
                    assert_eq!(evicted, expected, "step {step}");
                }
                // access a tracked id
                5..=7 => {
                    if model.is_empty() {
                        continue;
                    }
                    let pos = (next() as usize) % model.len();
                    let entry = model.remove(pos).unwrap();
                    touch(&mut p, entry.0);
                    model.push_front(entry);
                }
                // remove a tracked id
                8 => {
                    if model.is_empty() {
                        continue;
                    }
                    let pos = (next() as usize) % model.len();
                    let entry = model.remove(pos).unwrap();
                    remove(&mut p, entry.0);
                }
                // occasionally change the capacity
                _ => {
                    if next() % 10 != 0 {
                        continue;
                    }
                    capacity = (next() % 16) as usize;
                    let evicted = set_capacity(&mut p, capacity);
                    let mut expected = Vec::new();
                    while total(&model) > capacity {
                        expected.push(model.pop_back().unwrap().0 as usize);
                    }
                    assert_eq!(evicted, expected, "step {step}");
                }
            }

            let expected_order: Vec<usize> = model.iter().map(|e| e.0 as usize).collect();
            assert_eq!(order(&p), expected_order, "step {step}");
            assert_eq!(p.charge(), total(&model), "step {step}");
        }
    }
}
