use crate::comparator::Comparator;

pub struct BytewiseComparator;

const NAME: &str = "BYTEWISE";
impl Comparator for BytewiseComparator {
    fn compare(&self, a: &[u8], b: &[u8]) -> std::cmp::Ordering {
        a.cmp(b)
    }

    fn name(&self) -> &'static str {
        NAME
    }

    fn find_shortest_separator(&self, start: &mut Vec<u8>, limit: &[u8]) {
        let l = std::cmp::min(limit.len(), start.len());
        for i in 0..l {
            if start[i] < limit[i] && (limit[i] - start[i] != 1u8) {
                // ith byte is lesser and difference is > 1
                start[i] += 1;
                start.truncate(i + 1);
                return;
            }
        }
    }

    fn find_short_successor(&self, key: &mut Vec<u8>) {
        if !key.is_empty() && !(*key.first().unwrap() == 0xffu8) {
            *key.get_mut(0).unwrap() += 1;
            key.truncate(1);
        }
    }
}
