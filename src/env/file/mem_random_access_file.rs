use crate::env::RandomAccessFile;
use std::cmp::min;

pub struct MemRandomAccessFile {
    data: Vec<u8>,
}

impl MemRandomAccessFile {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }
}

impl RandomAccessFile for MemRandomAccessFile {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> crate::Result<usize> {
        if offset >= self.data.len() as u64 {
            return Ok(0);
        }
        let size = min(buf.len() as u64, self.data.len() as u64 - offset) as usize;
        buf[..size].copy_from_slice(&self.data[offset as usize..offset as usize + size]);
        Ok(size)
    }
}
