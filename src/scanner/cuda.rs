use anyhow::{anyhow, Result};

use crate::config::Config;
use crate::scanner::{Hit, SignatureScanner};
use crate::chunk::ScanChunk;

pub struct CudaScanner;

impl CudaScanner {
    pub fn new(_cfg: &Config) -> Result<Self> {
        Err(anyhow!("cuda scanner not implemented"))
    }
}

impl SignatureScanner for CudaScanner {
    fn scan_chunk(&self, _chunk: &ScanChunk, _data: &[u8]) -> Vec<Hit> {
        Vec::new()
    }
}
