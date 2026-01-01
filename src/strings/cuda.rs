use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use cudarc::driver::{CudaDevice, CudaFunction, CudaSlice, LaunchAsync, LaunchConfig};
use tracing::warn;

use crate::chunk::ScanChunk;
use crate::config::Config;
use crate::strings::cpu::CpuStringScanner;
use crate::strings::{StringScanner, StringSpan};

const KERNEL_SRC: &str = r#"
extern "C" __global__ void scan_ascii_spans(
    const unsigned char* data,
    unsigned long long data_len,
    unsigned int min_len,
    unsigned int max_len,
    unsigned int* span_starts,
    unsigned int* span_lens,
    unsigned int* span_flags,
    unsigned int* span_count,
    unsigned int max_spans) {

    unsigned long long gid = blockIdx.x * blockDim.x + threadIdx.x;
    if (gid >= data_len) {
        return;
    }
    unsigned char b = data[gid];
    if (!(b == 9 || (b >= 32 && b <= 126))) {
        return;
    }
    if (gid > 0) {
        unsigned char prev = data[gid - 1];
        if (prev == 9 || (prev >= 32 && prev <= 126)) {
            return;
        }
    }

    unsigned int len = 0;
    unsigned int digits = 0;
    unsigned int flags = 0;
    unsigned int window = 0;
    const unsigned int HTTP = 0x68747470;
    const unsigned int WWW = 0x7777772e;

    while ((gid + len) < data_len) {
        unsigned char c = data[gid + len];
        if (!(c == 9 || (c >= 32 && c <= 126))) {
            break;
        }
        unsigned char lower = c;
        if (lower >= 'A' && lower <= 'Z') {
            lower = lower + 32;
        }
        window = (window << 8) | (unsigned int)lower;
        if (window == HTTP || window == WWW) {
            flags |= 16;
        }
        if (c == '@') {
            flags |= 32;
        }
        if (c >= '0' && c <= '9') {
            digits += 1;
        }
        len += 1;
        if (len >= max_len) {
            break;
        }
    }

    if (digits >= 10) {
        flags |= 64;
    }
    if (len >= min_len) {
        unsigned int idx = atomicAdd(span_count, 1);
        if (idx < max_spans) {
            span_starts[idx] = (unsigned int)gid;
            span_lens[idx] = len;
            span_flags[idx] = flags;
        }
    }
}
"#;

const BLOCK_SIZE: u32 = 256;

pub struct CudaStringScanner {
    /// Mutex wraps the device to serialize kernel operations for thread safety.
    device: Mutex<Arc<CudaDevice>>,
    min_len: usize,
    max_len: usize,
    min_len_u32: u32,
    max_len_u32: u32,
    max_spans_per_chunk: u32,
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
            .load_ptx(ptx, "strings", &["scan_ascii_spans"])
            .map_err(|e| anyhow!("CUDA PTX load failed: {e}"))?;

        let max_len = if cfg.string_max_len == 0 {
            usize::MAX
        } else {
            cfg.string_max_len
        };
        let min_len_u32 = cfg.string_min_len.min(u32::MAX as usize) as u32;
        let max_len_u32 = if max_len > u32::MAX as usize {
            u32::MAX
        } else {
            max_len as u32
        };
        let max_spans_per_chunk = cfg
            .gpu_max_string_spans_per_chunk
            .min(u32::MAX as usize)
            .max(1) as u32;

        Ok(Self {
            device: Mutex::new(device),
            min_len: cfg.string_min_len,
            max_len,
            min_len_u32,
            max_len_u32,
            max_spans_per_chunk,
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

        let span_capacity = self.max_spans_per_chunk as usize;
        let starts_gpu: CudaSlice<u32> = match device.alloc_zeros(span_capacity) {
            Ok(buf) => buf,
            Err(err) => {
                warn!("CUDA span starts alloc failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };
        let lens_gpu: CudaSlice<u32> = match device.alloc_zeros(span_capacity) {
            Ok(buf) => buf,
            Err(err) => {
                warn!("CUDA span lengths alloc failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };
        let flags_gpu: CudaSlice<u32> = match device.alloc_zeros(span_capacity) {
            Ok(buf) => buf,
            Err(err) => {
                warn!("CUDA span flags alloc failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };
        let count_gpu: CudaSlice<u32> = match device.alloc_zeros(1) {
            Ok(buf) => buf,
            Err(err) => {
                warn!("CUDA span count alloc failed: {err}; using cpu fallback");
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
        let func: CudaFunction = match device.get_func("strings", "scan_ascii_spans") {
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
                    self.min_len_u32,
                    self.max_len_u32,
                    &starts_gpu,
                    &lens_gpu,
                    &flags_gpu,
                    &count_gpu,
                    self.max_spans_per_chunk,
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

        let count_host: Vec<u32> = match device.dtoh_sync_copy(&count_gpu) {
            Ok(v) => v,
            Err(err) => {
                warn!("CUDA span count read failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };
        let mut count = count_host[0] as usize;
        if count > span_capacity {
            warn!("CUDA span overflow: count={} max={}", count, span_capacity);
            count = span_capacity;
        }

        let starts: Vec<u32> = match device.dtoh_sync_copy(&starts_gpu) {
            Ok(v) => v,
            Err(err) => {
                warn!("CUDA span starts read failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };
        let lens: Vec<u32> = match device.dtoh_sync_copy(&lens_gpu) {
            Ok(v) => v,
            Err(err) => {
                warn!("CUDA span lengths read failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };
        let flags: Vec<u32> = match device.dtoh_sync_copy(&flags_gpu) {
            Ok(v) => v,
            Err(err) => {
                warn!("CUDA span flags read failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };

        let mut spans = Vec::with_capacity(count);
        for idx in 0..count {
            spans.push(StringSpan {
                chunk_id: chunk.id,
                local_start: starts[idx] as u64,
                length: lens[idx],
                flags: flags[idx],
            });
        }
        let mut spans = extend_long_ascii_spans(chunk, data, spans, self.min_len, self.max_len);
        let mut utf8 = crate::strings::cpu::scan_utf8_runs(data, chunk, self.min_len, self.max_len);
        spans.append(&mut utf8);
        if self.scan_utf16 {
            let mut utf16 =
                crate::strings::cpu::scan_utf16_runs(data, chunk, self.min_len, self.max_len, true);
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

fn extend_long_ascii_spans(
    chunk: &ScanChunk,
    data: &[u8],
    spans: Vec<StringSpan>,
    min_len: usize,
    max_len: usize,
) -> Vec<StringSpan> {
    if max_len == usize::MAX {
        return spans;
    }
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        let start = span.local_start as usize;
        let len = span.length as usize;
        out.push(span);
        if len < max_len {
            continue;
        }
        let mut idx = start + len;
        if idx >= data.len() || !is_printable(data[idx]) {
            continue;
        }
        while idx < data.len() && is_printable(data[idx]) {
            let run_start = idx;
            let mut run_len = 0usize;
            while idx < data.len() && is_printable(data[idx]) {
                idx += 1;
                run_len += 1;
                if run_len >= max_len {
                    break;
                }
            }
            if run_len >= min_len {
                let slice = &data[run_start..run_start + run_len];
                let flags = crate::strings::cpu::span_flags_ascii(slice);
                out.push(StringSpan {
                    chunk_id: chunk.id,
                    local_start: run_start as u64,
                    length: run_len as u32,
                    flags,
                });
            }
        }
    }
    out
}

fn is_printable(byte: u8) -> bool {
    matches!(byte, b'\t' | 0x20..=0x7E)
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
        msg.contains("CUDA_ERROR_NO_DEVICE") || msg.contains("no CUDA-capable device")
    }

    /// Check if tests should fail on any CUDA error (set FASTCARVE_REQUIRE_CUDA=1)
    fn require_cuda() -> bool {
        std::env::var("FASTCARVE_REQUIRE_CUDA")
            .map(|v| v == "1")
            .unwrap_or(false)
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

        let mut spans = scanner.scan_chunk(&chunk, data);
        spans.sort_by_key(|span| span.local_start);

        // Should find at least 2 strings
        assert!(
            spans.len() >= 2,
            "Should find at least 2 string spans, found {}",
            spans.len()
        );

        // First string "hello world" starts at offset 2
        assert_eq!(spans[0].local_start, 2);
        assert_eq!(spans[0].length, 11); // "hello world"

        // Second string "test string" starts at offset 16
        assert_eq!(spans[1].local_start, 16);
        assert_eq!(spans[1].length, 11); // "test string"
    }
}
