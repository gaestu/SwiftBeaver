pub mod cpu;
#[cfg(feature = "gpu-opencl")]
pub mod opencl;
#[cfg(feature = "gpu-cuda")]
pub mod cuda;

use crate::chunk::ScanChunk;

#[derive(Debug, Clone)]
pub struct Hit {
    pub chunk_id: u64,
    pub local_offset: u64,
    pub pattern_id: String,
    pub file_type_id: String,
}

#[derive(Debug, Clone)]
pub struct NormalizedHit {
    pub global_offset: u64,
    pub file_type_id: String,
    pub pattern_id: String,
}

pub trait SignatureScanner: Send + Sync {
    fn scan_chunk(&self, chunk: &ScanChunk, data: &[u8]) -> Vec<Hit>;
}

use crate::config::Config;
use anyhow::Result;
use tracing::warn;

pub fn build_signature_scanner(cfg: &Config, use_gpu: bool) -> Result<Box<dyn SignatureScanner>> {
    if use_gpu {
        #[cfg(feature = "gpu-opencl")]
        {
            match opencl::OpenClScanner::new(cfg) {
                Ok(scanner) => return Ok(Box::new(scanner)),
                Err(err) => warn!("opencl scanner init failed: {err}; falling back to cpu"),
            }
        }
        #[cfg(feature = "gpu-cuda")]
        {
            match cuda::CudaScanner::new(cfg) {
                Ok(scanner) => return Ok(Box::new(scanner)),
                Err(err) => warn!("cuda scanner init failed: {err}; falling back to cpu"),
            }
        }
        #[cfg(not(any(feature = "gpu-opencl", feature = "gpu-cuda")))]
        {
            warn!("gpu flag set but binary built without gpu backend, falling back to cpu");
        }
    }
    Ok(Box::new(cpu::CpuScanner::new(cfg)?))
}

#[cfg(test)]
mod tests {
    use super::build_signature_scanner;
    use crate::config;

    #[test]
    fn builds_scanner_with_gpu_flag() {
        let loaded = config::load_config(None).expect("config");
        let scanner = build_signature_scanner(&loaded.config, true).expect("scanner");
        let _ = scanner;
    }
}
