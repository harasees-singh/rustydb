use std::cmp::Ordering;

pub trait Comparator: Send + Sync {
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering;
    fn name(&self) -> &'static str;
    fn find_shortest_separator(&self, start: &mut Vec<u8>, limit: &[u8]) {}
    fn find_short_successor(&self, key: &mut Vec<u8>) {}
}
