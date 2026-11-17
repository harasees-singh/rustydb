pub struct BlockBuilder {
    restart_interval: usize,
    buffer: Vec<u8>,
    restarts: Vec<u32>,
    counter: usize,
    last_key: Vec<u8>,
    finished: bool
}

impl BlockBuilder {
    pub fn new(restart_interval: usize) -> Self {
        todo!()
    }
    pub fn add(&mut self, key: &[u8], value: &[u8]) {
        todo!()
    }
    pub fn finish(&mut self) -> &[u8] {
        todo!()
    }
    pub fn reset(&mut self) {
        todo!()
    }
    pub fn current_size_estimate(&self) -> usize {
        todo!()
    }
    pub fn is_empty(&self) -> bool {
        todo!()
    }
}