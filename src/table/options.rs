use crate::comparator::Comparator;
use crate::compression::CompressionType;
use crate::filter::traits::FilterPolicy;
use std::sync::Arc;
use crate::comparator::bytewise::BytewiseComparator;
use crate::filter::bloom::BloomFilterPolicy;

#[derive(Clone)]
pub struct TableOptions {
    pub comparator: Arc<dyn Comparator>,
    pub filter_policy: Option<Arc<dyn FilterPolicy>>,
    pub block_size: usize,
    pub restart_interval: usize,
    pub compression: CompressionType,
    pub verify_checksums: bool,
}

impl TableOptions {
    pub fn default() -> Self {
        Self {
            comparator: Arc::new(BytewiseComparator),
            filter_policy: Some(Arc::new(BloomFilterPolicy)),
            block_size: 4096, // 4 KB
            restart_interval: 16,
            compression: CompressionType::None,
            verify_checksums: true,
        }
    }
}