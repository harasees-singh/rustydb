use crate::error::Result;
pub trait WritableFile: Send {
    fn append(&mut self, data: &[u8]) -> Result<()>;
    fn flush(&mut self) -> Result<()>; // hand buffered bytes to the OS
    fn sync(&mut self) -> Result<()>; // fsync: bytes are durable on disk
    fn close(&mut self) -> Result<()>;
}
