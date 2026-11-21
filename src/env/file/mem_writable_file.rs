use crate::env::WritableFile;
use std::sync::{Arc, Mutex};

pub struct MemWritableFile {
    data: Arc<Mutex<Vec<u8>>>,
}

impl WritableFile for MemWritableFile {
    fn append(&mut self, data: &[u8]) -> crate::Result<()> {
        todo!()
    }

    fn flush(&mut self) -> crate::Result<()> {
        todo!()
    }

    fn sync(&mut self) -> crate::Result<()> {
        todo!()
    }

    fn close(&mut self) -> crate::Result<()> {
        todo!()
    }
}
