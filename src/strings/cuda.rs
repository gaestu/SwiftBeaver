use anyhow::{anyhow, Result};

use crate::chunk::ScanChunk;
use crate::config::Config;
use crate::strings::{StringScanner, StringSpan};

pub struct CudaStringScanner;

impl CudaStringScanner {
    pub fn new(_cfg: &Config) -> Result<Self> {
        Err(anyhow!("cuda string scanner not implemented"))
    }
}

impl StringScanner for CudaStringScanner {
    fn scan_chunk(&self, _chunk: &ScanChunk, _data: &[u8]) -> Vec<StringSpan> {
        Vec::new()
    }
}
