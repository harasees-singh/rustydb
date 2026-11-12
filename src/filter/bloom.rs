use crate::filter::traits::FilterPolicy;

pub struct BloomFilterPolicy {}
impl FilterPolicy for BloomFilterPolicy {
    fn name(&self) -> &'static str {
        todo!()
    }

    fn create_filter(&self, keys: &[&[u8]], dst: &mut Vec<u8>) {
        todo!()
    }

    fn key_may_match(&self, key: &[u8], filter: &[u8]) -> bool {
        todo!()
    }
}
