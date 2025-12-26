use std::ptr;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use opencl3::command_queue::{CommandQueue, CL_BLOCKING};
use opencl3::context::Context;
use opencl3::device::{Device, CL_DEVICE_TYPE_GPU};
use opencl3::kernel::Kernel;
use opencl3::memory::{
    Buffer,
    ClMem,
    CL_MEM_COPY_HOST_PTR,
    CL_MEM_READ_ONLY,
    CL_MEM_READ_WRITE,
    CL_MEM_WRITE_ONLY,
};
use opencl3::platform::get_platforms;
use opencl3::program::Program;
use opencl3::types::{cl_uint, cl_ulong};
use tracing::warn;

use crate::chunk::ScanChunk;
use crate::config::Config;
use crate::scanner::cpu::CpuScanner;
use crate::scanner::{Hit, SignatureScanner};

const KERNEL_SRC: &str = r#"
#pragma OPENCL EXTENSION cl_khr_global_int32_base_atomics : enable
__kernel void scan_pattern(
    __global const uchar* data,
    ulong data_len,
    __global const uchar* pattern,
    uint pat_len,
    __global uint* hits,
    __global uint* hit_count,
    uint max_hits) {
    size_t gid = get_global_id(0);
    if (gid + pat_len > data_len) {
        return;
    }
    for (uint i = 0; i < pat_len; i++) {
        if (data[gid + i] != pattern[i]) {
            return;
        }
    }
    uint idx = atomic_inc(hit_count);
    if (idx < max_hits) {
        hits[idx] = (uint) gid;
    }
}
"#;

const MAX_PATTERN_LEN: usize = 32;

#[derive(Debug, Clone)]
struct Pattern {
    id: String,
    file_type_id: String,
    bytes: Vec<u8>,
}

pub struct OpenClScanner {
    context: Context,
    queue: CommandQueue,
    kernel: Mutex<Kernel>,
    patterns: Vec<Pattern>,
    max_hits_per_chunk: u32,
    cpu_fallback: CpuScanner,
}

impl OpenClScanner {
    pub fn new(cfg: &Config) -> Result<Self> {
        let patterns = parse_patterns(cfg)?;
        let cpu_fallback = CpuScanner::new(cfg)?;

        if patterns.is_empty() {
            return Err(anyhow!("no patterns configured"));
        }
        if patterns.iter().any(|p| p.bytes.len() > MAX_PATTERN_LEN) {
            return Err(anyhow!("pattern length exceeds max for OpenCL"));
        }

        let (_device, context) = select_device(cfg)?;
        #[allow(deprecated)]
        let queue = CommandQueue::create_default(&context, 0)?;
        let program = Program::create_and_build_from_source(&context, KERNEL_SRC, "")
            .map_err(|err| anyhow!(err))?;
        let kernel = Kernel::create(&program, "scan_pattern")?;

        let max_hits = cfg
            .gpu_max_hits_per_chunk
            .min(u32::MAX as usize)
            .max(1) as u32;

        Ok(Self {
            context,
            queue,
            kernel: Mutex::new(kernel),
            patterns,
            max_hits_per_chunk: max_hits,
            cpu_fallback,
        })
    }
}

impl SignatureScanner for OpenClScanner {
    fn scan_chunk(&self, chunk: &ScanChunk, data: &[u8]) -> Vec<Hit> {
        if data.is_empty() {
            return Vec::new();
        }
        if data.len() > u32::MAX as usize {
            warn!("chunk length exceeds u32::MAX; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }
        if self.patterns.iter().any(|p| p.bytes.len() > MAX_PATTERN_LEN) {
            warn!("pattern length exceeds OpenCL cap; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }

        let mut hits = Vec::new();
        let data_len = data.len() as cl_ulong;

        for pattern in &self.patterns {
            let pat_len = pattern.bytes.len() as cl_uint;
            if pat_len == 0 {
                continue;
            }
            if data_len < pat_len as cl_ulong {
                continue;
            }

            let data_buffer = match unsafe {
                Buffer::<u8>::create(
                    &self.context,
                    CL_MEM_READ_ONLY | CL_MEM_COPY_HOST_PTR,
                    data.len(),
                    data.as_ptr() as *mut _,
                )
            } {
                Ok(buf) => buf,
                Err(err) => {
                    warn!("opencl buffer create failed: {err}; using cpu fallback");
                    return self.cpu_fallback.scan_chunk(chunk, data);
                }
            };

            let pattern_buffer = match unsafe {
                Buffer::<u8>::create(
                    &self.context,
                    CL_MEM_READ_ONLY | CL_MEM_COPY_HOST_PTR,
                    pattern.bytes.len(),
                    pattern.bytes.as_ptr() as *mut _,
                )
            } {
                Ok(buf) => buf,
                Err(err) => {
                    warn!("opencl pattern buffer create failed: {err}; using cpu fallback");
                    return self.cpu_fallback.scan_chunk(chunk, data);
                }
            };

            let hits_buffer = match unsafe {
                Buffer::<cl_uint>::create(
                    &self.context,
                    CL_MEM_WRITE_ONLY,
                    self.max_hits_per_chunk as usize,
                    ptr::null_mut(),
                )
            } {
                Ok(buf) => buf,
                Err(err) => {
                    warn!("opencl hits buffer create failed: {err}; using cpu fallback");
                    return self.cpu_fallback.scan_chunk(chunk, data);
                }
            };

            let mut zero = [0u32];
            let count_buffer = match unsafe {
                Buffer::<cl_uint>::create(
                    &self.context,
                    CL_MEM_READ_WRITE | CL_MEM_COPY_HOST_PTR,
                    1,
                    zero.as_mut_ptr() as *mut _,
                )
            } {
                Ok(buf) => buf,
                Err(err) => {
                    warn!("opencl count buffer create failed: {err}; using cpu fallback");
                    return self.cpu_fallback.scan_chunk(chunk, data);
                }
            };

            let kernel = match self.kernel.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };

            let data_mem = data_buffer.get();
            let pattern_mem = pattern_buffer.get();
            let hits_mem = hits_buffer.get();
            let count_mem = count_buffer.get();

            if let Err(err) = unsafe { kernel.set_arg(0, &data_mem) } {
                warn!("opencl kernel arg error: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
            let _ = unsafe { kernel.set_arg(1, &data_len) };
            let _ = unsafe { kernel.set_arg(2, &pattern_mem) };
            let _ = unsafe { kernel.set_arg(3, &pat_len) };
            let _ = unsafe { kernel.set_arg(4, &hits_mem) };
            let _ = unsafe { kernel.set_arg(5, &count_mem) };
            let _ = unsafe { kernel.set_arg(6, &(self.max_hits_per_chunk as cl_uint)) };

            let global_work_size = [data.len() as usize];
            if let Err(err) = unsafe {
                self.queue.enqueue_nd_range_kernel(
                    kernel.get(),
                    1,
                    ptr::null(),
                    global_work_size.as_ptr(),
                    ptr::null(),
                    &[],
                )
            } {
                warn!("opencl kernel launch failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }

            if let Err(err) = self.queue.finish() {
                warn!("opencl queue finish failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }

            if let Err(err) = unsafe {
                self.queue
                    .enqueue_read_buffer(&count_buffer, CL_BLOCKING, 0, &mut zero, &[])
            } {
                warn!("opencl read count failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }

            let mut count = zero[0] as usize;
            if count > self.max_hits_per_chunk as usize {
                warn!("opencl hits overflow: count={} max={}", count, self.max_hits_per_chunk);
                count = self.max_hits_per_chunk as usize;
            }

            if count == 0 {
                continue;
            }

            let mut offsets = vec![0u32; count];
            if let Err(err) = unsafe {
                self.queue
                    .enqueue_read_buffer(&hits_buffer, CL_BLOCKING, 0, &mut offsets, &[])
            } {
                warn!("opencl read hits failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }

            for offset in offsets {
                hits.push(Hit {
                    chunk_id: chunk.id,
                    local_offset: offset as u64,
                    pattern_id: pattern.id.clone(),
                    file_type_id: pattern.file_type_id.clone(),
                });
            }
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

fn select_device(cfg: &Config) -> Result<(Device, Context)> {
    let platforms = get_platforms()?;
    if platforms.is_empty() {
        return Err(anyhow!("no OpenCL platforms available"));
    }

    if let (Some(platform_idx), Some(device_idx)) = (cfg.opencl_platform_index, cfg.opencl_device_index) {
        if platform_idx >= platforms.len() {
            return Err(anyhow!("opencl platform index out of range"));
        }
        let platform = platforms[platform_idx];
        let devices = platform.get_devices(CL_DEVICE_TYPE_GPU)?;
        if device_idx >= devices.len() {
            return Err(anyhow!("opencl device index out of range"));
        }
        let device = Device::new(devices[device_idx]);
        let context = Context::from_device(&device)?;
        return Ok((device, context));
    }

    for platform in platforms {
        let devices = platform.get_devices(CL_DEVICE_TYPE_GPU)?;
        if let Some(device_id) = devices.first() {
            let device = Device::new(*device_id);
            let context = Context::from_device(&device)?;
            return Ok((device, context));
        }
    }

    Err(anyhow!("no OpenCL GPU device found"))
}
