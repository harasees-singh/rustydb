use crate::env::WritableFile;
use std::sync::{Arc, Mutex};

pub struct MemWritableFile {
    data: Arc<Mutex<Vec<u8>>>,
}

impl MemWritableFile {
    pub fn new(data: Arc<Mutex<Vec<u8>>>) -> Self {
        Self {
            data: Arc::clone(&data),
        }
    }
}

impl WritableFile for MemWritableFile {
    fn append(&mut self, data: &[u8]) -> crate::Result<()> {
        self.data.lock().unwrap().extend_from_slice(data);
        Ok(())
    }

    fn flush(&mut self) -> crate::Result<()> {
        Ok(())
    }

    fn sync(&mut self) -> crate::Result<()> {
        Ok(())
    }

    fn close(self: Box<Self>) -> crate::Result<()> {
        Ok(())
    }
}
