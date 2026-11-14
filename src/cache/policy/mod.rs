//! Eviction policies: the bookkeeping that decides which entries to drop.
//!
//! A policy never sees keys or values. It tracks only [`EntryId`]s (slots owned
//! by the cache), their charges and enough history to pick victims. That keeps
//! policies small and swappable, and lets them be tested and measured by
//! replaying access traces, with no values and no threads.

pub mod policy_impl;
pub mod traits;
pub use traits::EvictionPolicy;

/// Identifies one entry inside a single cache shard.
///
/// It is a slab index handed out by the shard, so ids are small, dense and
/// reused after an entry is removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntryId(u32);

impl EntryId {
    pub fn new(index: u32) -> Self {
        EntryId(index)
    }

    /// The slab index, for policies that keep per-entry state in a `Vec`.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}
