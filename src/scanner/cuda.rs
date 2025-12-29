use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use cudarc::driver::{CudaDevice, CudaFunction, CudaSlice, LaunchAsync, LaunchConfig};
use tracing::warn;

use crate::chunk::ScanChunk;
use crate::config::Config;
use crate::scanner::cpu::CpuScanner;
use crate::scanner::{Hit, SignatureScanner};

const KERNEL_SRC: &str = r#"
extern "C" __global__ void scan_patterns(
    const unsigned char* data,
    unsigned long long data_len,
    const unsigned char* patterns,
    const unsigned int* pattern_offsets,
    const unsigned int* pattern_lengths,
    unsigned int pattern_count,
    unsigned int* hit_offsets,
    unsigned int* hit_pattern_ids,
    unsigned int* hit_count,
    unsigned int max_hits) {

    unsigned long long gid = blockIdx.x * blockDim.x + threadIdx.x;
    if (gid >= data_len) {
        return;
    }

    for (unsigned int p = 0; p < pattern_count; p++) {
        unsigned int pat_len = pattern_lengths[p];
        if (pat_len == 0 || (gid + pat_len) > data_len) {
            continue;
        }
        unsigned int pat_off = pattern_offsets[p];
        unsigned int matched = 1;
        for (unsigned int i = 0; i < pat_len; i++) {
            if (data[gid + i] != patterns[pat_off + i]) {
                matched = 0;
                break;
            }
        }
        if (matched != 0) {
            unsigned int idx = atomicAdd(hit_count, 1);
            if (idx < max_hits) {
                hit_offsets[idx] = (unsigned int)gid;
                hit_pattern_ids[idx] = p;
            }
        }
    }
}
"#;
const BLOCK_SIZE: u32 = 256;

#[derive(Debug, Clone)]
struct Pattern {
    id: String,
    file_type_id: String,
    bytes: Vec<u8>,
}

pub struct CudaScanner {
    /// Mutex wraps the device to serialize kernel operations for thread safety.
    /// While CudaDevice is Send+Sync, kernel launches should be serialized.
    device: Mutex<Arc<CudaDevice>>,
    patterns: Vec<Pattern>,
    pattern_count: u32,
    pattern_bytes: CudaSlice<u8>,
    pattern_offsets: CudaSlice<u32>,
    pattern_lengths: CudaSlice<u32>,
    max_hits_per_chunk: u32,
    cpu_fallback: CpuScanner,
}

impl CudaScanner {
    pub fn new(cfg: &Config) -> Result<Self> {
        let patterns = parse_patterns(cfg)?;
        let cpu_fallback = CpuScanner::new(cfg)?;

        if patterns.is_empty() {
            return Err(anyhow!("no patterns configured"));
        }

        let device = CudaDevice::new(0).map_err(|e| anyhow!("CUDA device init failed: {e}"))?;

        // Compile the kernel
        let ptx = cudarc::nvrtc::compile_ptx(KERNEL_SRC)
            .map_err(|e| anyhow!("CUDA kernel compilation failed: {e}"))?;
        
        device
            .load_ptx(ptx, "scanner", &["scan_patterns"])
            .map_err(|e| anyhow!("CUDA PTX load failed: {e}"))?;

        let (pattern_bytes, pattern_offsets, pattern_lengths) = build_pattern_buffers(&patterns)?;
        let pattern_count = patterns.len() as u32;
        let pattern_bytes = device
            .htod_copy(pattern_bytes)
            .map_err(|e| anyhow!("CUDA pattern bytes copy failed: {e}"))?;
        let pattern_offsets = device
            .htod_copy(pattern_offsets)
            .map_err(|e| anyhow!("CUDA pattern offsets copy failed: {e}"))?;
        let pattern_lengths = device
            .htod_copy(pattern_lengths)
            .map_err(|e| anyhow!("CUDA pattern lengths copy failed: {e}"))?;

        let max_hits = cfg
            .gpu_max_hits_per_chunk
            .min(u32::MAX as usize)
            .max(1) as u32;

        Ok(Self {
            device: Mutex::new(device),
            patterns,
            pattern_count,
            pattern_bytes,
            pattern_offsets,
            pattern_lengths,
            max_hits_per_chunk: max_hits,
            cpu_fallback,
        })
    }
}

impl SignatureScanner for CudaScanner {
    fn scan_chunk(&self, chunk: &ScanChunk, data: &[u8]) -> Vec<Hit> {
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

        // Copy data to device once
        let data_gpu: CudaSlice<u8> = match device.htod_copy(data.to_vec()) {
            Ok(buf) => buf,
            Err(err) => {
                warn!("CUDA data copy failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };

        let hits_gpu: CudaSlice<u32> = match device.alloc_zeros(self.max_hits_per_chunk as usize) {
            Ok(buf) => buf,
            Err(err) => {
                warn!("CUDA hits alloc failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };
        let hit_patterns_gpu: CudaSlice<u32> =
            match device.alloc_zeros(self.max_hits_per_chunk as usize) {
                Ok(buf) => buf,
                Err(err) => {
                    warn!("CUDA hit pattern alloc failed: {err}; using cpu fallback");
                    return self.cpu_fallback.scan_chunk(chunk, data);
                }
            };
        let count_gpu: CudaSlice<u32> = match device.alloc_zeros(1) {
            Ok(buf) => buf,
            Err(err) => {
                warn!("CUDA count alloc failed: {err}; using cpu fallback");
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
        let func: CudaFunction = match device.get_func("scanner", "scan_patterns") {
            Some(f) => f,
            None => {
                warn!("CUDA kernel not found; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };

        // Launch kernel
        let launch_result = unsafe {
            func.launch(
                launch_cfg,
                (
                    &data_gpu,
                    data_len,
                    &self.pattern_bytes,
                    &self.pattern_offsets,
                    &self.pattern_lengths,
                    self.pattern_count,
                    &hits_gpu,
                    &hit_patterns_gpu,
                    &count_gpu,
                    self.max_hits_per_chunk,
                ),
            )
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

        // Read back hit count
        let count_host = match device.dtoh_sync_copy(&count_gpu) {
            Ok(v) => v,
            Err(err) => {
                warn!("CUDA count read failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };

        let mut count = count_host[0] as usize;
        if count > self.max_hits_per_chunk as usize {
            warn!(
                "CUDA hits overflow: count={} max={}",
                count, self.max_hits_per_chunk
            );
            count = self.max_hits_per_chunk as usize;
        }
        if count == 0 {
            return Vec::new();
        }

        let hits_host: Vec<u32> = match device.dtoh_sync_copy(&hits_gpu) {
            Ok(v) => v,
            Err(err) => {
                warn!("CUDA hits read failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };
        let hit_patterns_host: Vec<u32> = match device.dtoh_sync_copy(&hit_patterns_gpu) {
            Ok(v) => v,
            Err(err) => {
                warn!("CUDA hit patterns read failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };

        let mut hits = Vec::with_capacity(count);
        for idx in 0..count {
            let pattern_idx = hit_patterns_host[idx] as usize;
            let Some(pattern) = self.patterns.get(pattern_idx) else {
                continue;
            };
            hits.push(Hit {
                chunk_id: chunk.id,
                local_offset: hits_host[idx] as u64,
                pattern_id: pattern.id.clone(),
                file_type_id: pattern.file_type_id.clone(),
            });
        }

        hits
    }
}

fn parse_patterns(cfg: &Config) -> Result<Vec<Pattern>> {
    let mut patterns = Vec::new();
    for file_type in &cfg.file_types {
        for pat in &file_type.header_patterns {
            let bytes = hex::decode(pat.hex.trim())
                .map_err(|e| anyhow!("invalid hex pattern {}: {e}", pat.id))?;
            if bytes.is_empty() {
                continue;
            }
            patterns.push(Pattern {
                id: pat.id.clone(),
                file_type_id: file_type.id.clone(),
                bytes,
            });
        }
    }
    Ok(patterns)
}

fn build_pattern_buffers(patterns: &[Pattern]) -> Result<(Vec<u8>, Vec<u32>, Vec<u32>)> {
    let mut flat = Vec::new();
    let mut offsets = Vec::with_capacity(patterns.len());
    let mut lengths = Vec::with_capacity(patterns.len());
    let mut cursor: u64 = 0;

    for pattern in patterns {
        let len = pattern.bytes.len();
        if len == 0 {
            continue;
        }
        if cursor + len as u64 > u32::MAX as u64 {
            return Err(anyhow!("pattern bytes exceed u32::MAX"));
        }
        offsets.push(cursor as u32);
        lengths.push(len as u32);
        flat.extend_from_slice(&pattern.bytes);
        cursor += len as u64;
    }

    Ok((flat, offsets, lengths))
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
    fn cuda_scanner_creates_successfully() {
        let loaded = crate::config::load_config(None).expect("config");
        let scanner = CudaScanner::new(&loaded.config);
        match &scanner {
            Ok(_) => eprintln!("CUDA scanner created successfully"),
            Err(e) if is_no_device_error(e) && !require_cuda() => {
                eprintln!("Skipping: no CUDA device available: {e}");
            }
            Err(e) => {
                panic!("CUDA scanner creation failed with unexpected error: {e}");
            }
        }
    }

    #[test]
    fn cuda_scanner_scans_chunk_with_pattern() {
        let loaded = crate::config::load_config(None).expect("config");
        let scanner = match CudaScanner::new(&loaded.config) {
            Ok(s) => s,
            Err(e) if is_no_device_error(&e) && !require_cuda() => {
                eprintln!("Skipping: no CUDA device available: {e}");
                return;
            }
            Err(e) => {
                panic!("CUDA scanner creation failed with unexpected error: {e}");
            }
        };

        // Create a small test buffer with JPEG magic bytes
        let mut data = vec![0u8; 1024];
        // JPEG magic: FF D8 FF E0
        data[100] = 0xFF;
        data[101] = 0xD8;
        data[102] = 0xFF;
        data[103] = 0xE0;

        let chunk = crate::chunk::ScanChunk {
            id: 0,
            start: 0,
            length: data.len() as u64,
            valid_length: data.len() as u64,
        };

        let hits = scanner.scan_chunk(&chunk, &data);
        // Should find the JPEG header
        let mut jpeg_hits: Vec<_> = hits.iter().filter(|h| h.file_type_id == "jpeg").collect();
        jpeg_hits.sort_by_key(|hit| hit.local_offset);
        assert!(!jpeg_hits.is_empty(), "Should find JPEG pattern");
        assert_eq!(jpeg_hits[0].local_offset, 100);
    }
}
