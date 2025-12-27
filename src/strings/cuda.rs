use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use cudarc::driver::{CudaDevice, CudaFunction, CudaSlice, LaunchAsync, LaunchConfig};
use tracing::warn;

use crate::chunk::ScanChunk;
use crate::config::Config;
use crate::strings::cpu::CpuStringScanner;
use crate::strings::{StringScanner, StringSpan};

const KERNEL_SRC: &str = r#"
extern "C" __global__ void mark_printable(
    const unsigned char* data,
    unsigned long long data_len,
    unsigned char* mask) {
    
    unsigned long long gid = blockIdx.x * blockDim.x + threadIdx.x;
    if (gid >= data_len) {
        return;
    }
    
    unsigned char b = data[gid];
    if (b == 9 || (b >= 32 && b <= 126)) {
        mask[gid] = 1;
    } else {
        mask[gid] = 0;
    }
}
"#;

const BLOCK_SIZE: u32 = 256;

pub struct CudaStringScanner {
    /// Mutex wraps the device to serialize kernel operations for thread safety.
    device: Mutex<Arc<CudaDevice>>,
    min_len: usize,
    max_len: usize,
    scan_utf16: bool,
    cpu_fallback: CpuStringScanner,
}

impl CudaStringScanner {
    pub fn new(cfg: &Config) -> Result<Self> {
        let device = CudaDevice::new(0).map_err(|e| anyhow!("CUDA device init failed: {e}"))?;

        // Compile the kernel
        let ptx = cudarc::nvrtc::compile_ptx(KERNEL_SRC)
            .map_err(|e| anyhow!("CUDA kernel compilation failed: {e}"))?;
        
        device
            .load_ptx(ptx, "strings", &["mark_printable"])
            .map_err(|e| anyhow!("CUDA PTX load failed: {e}"))?;

        let max_len = if cfg.string_max_len == 0 {
            usize::MAX
        } else {
            cfg.string_max_len
        };

        Ok(Self {
            device: Mutex::new(device),
            min_len: cfg.string_min_len,
            max_len,
            scan_utf16: cfg.string_scan_utf16,
            cpu_fallback: CpuStringScanner::new(
                cfg.string_min_len,
                cfg.string_max_len,
                cfg.string_scan_utf16,
            ),
        })
    }
}

impl StringScanner for CudaStringScanner {
    fn scan_chunk(&self, chunk: &ScanChunk, data: &[u8]) -> Vec<StringSpan> {
        if data.is_empty() {
            return Vec::new();
        }
        if data.len() > u32::MAX as usize {
            warn!("chunk length exceeds u32::MAX; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }

        // Acquire device lock for thread-safe GPU operations
        let device: std::sync::MutexGuard<'_, Arc<CudaDevice>> = match self.device.lock() {
            Ok(d) => d,
            Err(err) => {
                warn!("CUDA device lock poisoned: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };

        let data_len = data.len() as u64;

        // Copy data to device
        let data_gpu: CudaSlice<u8> = match device.htod_copy(data.to_vec()) {
            Ok(buf) => buf,
            Err(err) => {
                warn!("CUDA data copy failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };

        // Allocate mask buffer
        let mask_gpu: CudaSlice<u8> = match device.alloc_zeros(data.len()) {
            Ok(buf) => buf,
            Err(err) => {
                warn!("CUDA mask alloc failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };

        // Calculate grid dimensions
        let num_threads = data.len() as u32;
        let num_blocks = (num_threads + BLOCK_SIZE - 1) / BLOCK_SIZE;
        let launch_cfg = LaunchConfig {
            grid_dim: (num_blocks, 1, 1),
            block_dim: (BLOCK_SIZE, 1, 1),
            shared_mem_bytes: 0,
        };

        // Get the kernel function
        let func: CudaFunction = match device.get_func("strings", "mark_printable") {
            Some(f) => f,
            None => {
                warn!("CUDA kernel not found; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };

        // Launch kernel
        let launch_result = unsafe {
            func.launch(launch_cfg, (&data_gpu, data_len, &mask_gpu))
        };

        if let Err(err) = launch_result {
            warn!("CUDA kernel launch failed: {err}; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }

        // Synchronize
        if let Err(err) = device.synchronize() {
            warn!("CUDA synchronize failed: {err}; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }

        // Read back mask
        let mask: Vec<u8> = match device.dtoh_sync_copy(&mask_gpu) {
            Ok(v) => v,
            Err(err) => {
                warn!("CUDA mask read failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };

        // Convert mask to spans on CPU
        let mut spans = mask_to_spans(chunk, data, &mask, self.min_len, self.max_len);
        let mut utf8 = crate::strings::cpu::scan_utf8_runs(
            data,
            chunk,
            self.min_len,
            self.max_len,
        );
        spans.append(&mut utf8);
        if self.scan_utf16 {
            let mut utf16 = crate::strings::cpu::scan_utf16_runs(
                data,
                chunk,
                self.min_len,
                self.max_len,
                true,
            );
            spans.append(&mut utf16);
            let mut utf16 = crate::strings::cpu::scan_utf16_runs(
                data,
                chunk,
                self.min_len,
                self.max_len,
                false,
            );
            spans.append(&mut utf16);
        }
        spans
    }
}

fn mask_to_spans(
    chunk: &ScanChunk,
    data: &[u8],
    mask: &[u8],
    min_len: usize,
    max_len: usize,
) -> Vec<StringSpan> {
    let mut spans = Vec::new();
    let mut i = 0usize;

    while i < mask.len() {
        if mask[i] == 0 {
            i += 1;
            continue;
        }

        let start = i;
        let mut len = 0usize;
        while i < mask.len() && mask[i] != 0 {
            i += 1;
            len += 1;
            if len >= max_len {
                break;
            }
        }

        if len >= min_len {
            let slice = &data[start..start + len];
            let flags = crate::strings::cpu::span_flags_ascii(slice);
            spans.push(StringSpan {
                chunk_id: chunk.id,
                local_start: start as u64,
                length: len as u32,
                flags,
            });
        }

        // If we hit max_len, continue from current position (already incremented)
        // to find more spans
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns true if the error indicates no CUDA device is available.
    /// Only skips on actual "no device" errors - other errors (NVRTC failures,
    /// kernel load issues, driver misconfigurations) will fail the test.
    fn is_no_device_error(err: &anyhow::Error) -> bool {
        let msg = err.to_string();
        // CUDA_ERROR_NO_DEVICE (code 100) produces "no CUDA-capable device"
        // cudarc formats this as "CUDA_ERROR_NO_DEVICE" or "no CUDA-capable device"
        msg.contains("CUDA_ERROR_NO_DEVICE")
            || msg.contains("no CUDA-capable device")
    }

    /// Check if tests should fail on any CUDA error (set FASTCARVE_REQUIRE_CUDA=1)
    fn require_cuda() -> bool {
        std::env::var("FASTCARVE_REQUIRE_CUDA").map(|v| v == "1").unwrap_or(false)
    }

    #[test]
    fn cuda_string_scanner_creates_successfully() {
        let loaded = crate::config::load_config(None).expect("config");
        let scanner = CudaStringScanner::new(&loaded.config);
        match &scanner {
            Ok(_) => eprintln!("CUDA string scanner created successfully"),
            Err(e) if is_no_device_error(e) && !require_cuda() => {
                eprintln!("Skipping: no CUDA device available: {e}");
            }
            Err(e) => {
                panic!("CUDA string scanner creation failed with unexpected error: {e}");
            }
        }
    }

    #[test]
    fn cuda_string_scanner_finds_printable_runs() {
        let loaded = crate::config::load_config(None).expect("config");
        let scanner = match CudaStringScanner::new(&loaded.config) {
            Ok(s) => s,
            Err(e) if is_no_device_error(&e) && !require_cuda() => {
                eprintln!("Skipping: no CUDA device available: {e}");
                return;
            }
            Err(e) => {
                panic!("CUDA string scanner creation failed with unexpected error: {e}");
            }
        };

        // Create test data with printable strings separated by null bytes
        let data = b"\x00\x00hello world\x00\x00\x00test string\x00\x00";

        let chunk = crate::chunk::ScanChunk {
            id: 0,
            start: 0,
            length: data.len() as u64,
            valid_length: data.len() as u64,
        };

        let spans = scanner.scan_chunk(&chunk, data);
        
        // Should find at least 2 strings
        assert!(spans.len() >= 2, "Should find at least 2 string spans, found {}", spans.len());
        
        // First string "hello world" starts at offset 2
        assert_eq!(spans[0].local_start, 2);
        assert_eq!(spans[0].length, 11); // "hello world"
        
        // Second string "test string" starts at offset 16
        assert_eq!(spans[1].local_start, 16);
        assert_eq!(spans[1].length, 11); // "test string"
    }
}
