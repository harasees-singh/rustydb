use crate::comparator::Comparator;

pub struct BytewiseComparator;

const NAME: &str = "BYTEWISE";
impl Comparator for BytewiseComparator {
    fn compare(&self, a: &[u8], b: &[u8]) -> std::cmp::Ordering {
        let l = std::cmp::min(a.len(), b.len());
        for i in 0..l {
            if a[i] == b[i] {
                continue;
            }
            if a[i] < b[i] {
                return std::cmp::Ordering::Less;
            }
            return std::cmp::Ordering::Greater;
        }
        if a.len() == b.len() {
            return std::cmp::Ordering::Equal
        }
        if a.len() < b.len() {
            return std::cmp::Ordering::Less
        }
        std::cmp::Ordering::Greater
    }

    fn name(&self) -> &'static str {
        NAME
    }
}