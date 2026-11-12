pub trait FilterPolicy {
    fn name(&self) -> &'static str;
    fn create_filter(&self, keys: &[&[u8]], dst: &mut Vec<u8>);
    fn key_may_match(&self, key: &[u8], filter: &[u8]) -> bool;
}