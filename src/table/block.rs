use std::range::Range;
use std::sync::Arc;
use crate::comparator::Comparator;

pub struct Block {
    data: Vec<u8>,
    restarts_offset: usize,
    restarts_count: u32
}

impl Block {
    pub fn new(data: Vec<u8>) -> crate::Result<Block> {
        todo!()
    }

    pub fn size(&self) -> usize {
        todo!()
    }

    pub fn iter(self: &Arc<Self>, cmp: Arc<dyn Comparator>) -> BlockIter {
        todo!()
    }
}

pub struct BlockIter {
    block: Arc<Block>,
    cmp: Arc<dyn Comparator>,
    current: usize,
    restart_index: usize,
    key: Vec<u8>,
    value: Range<usize>,
    status: crate::Result<()>
}

impl crate::iterator::traits::Cursor for BlockIter {
    fn valid(&self) -> bool {
        todo!()
    }

    fn seek_to_first(&mut self) {
        todo!()
    }

    fn seek_to_last(&mut self) {
        todo!()
    }

    fn seek(&mut self, target: &Self) {
        todo!()
    }

    fn next(&mut self) {
        todo!()
    }

    fn prev(&mut self) {
        todo!()
    }

    fn key(&self) -> &[u8] {
        todo!()
    }

    fn value(&self) -> &[u8] {
        todo!()
    }

    fn status(&self) -> crate::Result<()> {
        todo!()
    }
}