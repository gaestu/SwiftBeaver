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
__kernel void scan_patterns(
    __global const uchar* data,
    ulong data_len,
    __global const uchar* patterns,
    __global const uint* pattern_offsets,
    __global const uint* pattern_lengths,
    uint pattern_count,
    __global uint* hit_offsets,
    __global uint* hit_pattern_ids,
    __global uint* hit_count,
    uint max_hits) {
    size_t gid = get_global_id(0);
    if (gid >= data_len) {
        return;
    }
    for (uint p = 0; p < pattern_count; p++) {
        uint pat_len = pattern_lengths[p];
        if (pat_len == 0 || gid + pat_len > data_len) {
            continue;
        }
        uint pat_off = pattern_offsets[p];
        uint matched = 1;
        for (uint i = 0; i < pat_len; i++) {
            if (data[gid + i] != patterns[pat_off + i]) {
                matched = 0;
                break;
            }
        }
        if (matched != 0) {
            uint idx = atomic_inc(hit_count);
            if (idx < max_hits) {
                hit_offsets[idx] = (uint)gid;
                hit_pattern_ids[idx] = p;
            }
        }
    }
}
"#;

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
    pattern_count: u32,
    pattern_bytes: Buffer<u8>,
    pattern_offsets: Buffer<cl_uint>,
    pattern_lengths: Buffer<cl_uint>,
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

        let (pattern_bytes, pattern_offsets, pattern_lengths) = build_pattern_buffers(&patterns)?;
        let pattern_count = patterns.len() as u32;

        let (_device, context) = select_device(cfg)?;
        #[allow(deprecated)]
        let queue = CommandQueue::create_default(&context, 0)?;
        let program = Program::create_and_build_from_source(&context, KERNEL_SRC, "")
            .map_err(|err| anyhow!(err))?;
        let kernel = Kernel::create(&program, "scan_patterns")?;

        let pattern_bytes_buffer = unsafe {
            Buffer::<u8>::create(
                &context,
                CL_MEM_READ_ONLY | CL_MEM_COPY_HOST_PTR,
                pattern_bytes.len(),
                pattern_bytes.as_ptr() as *mut _,
            )
        }
        .map_err(|err| anyhow!(err))?;
        let pattern_offsets_buffer = unsafe {
            Buffer::<cl_uint>::create(
                &context,
                CL_MEM_READ_ONLY | CL_MEM_COPY_HOST_PTR,
                pattern_offsets.len(),
                pattern_offsets.as_ptr() as *mut _,
            )
        }
        .map_err(|err| anyhow!(err))?;
        let pattern_lengths_buffer = unsafe {
            Buffer::<cl_uint>::create(
                &context,
                CL_MEM_READ_ONLY | CL_MEM_COPY_HOST_PTR,
                pattern_lengths.len(),
                pattern_lengths.as_ptr() as *mut _,
            )
        }
        .map_err(|err| anyhow!(err))?;

        let max_hits = cfg
            .gpu_max_hits_per_chunk
            .min(u32::MAX as usize)
            .max(1) as u32;

        Ok(Self {
            context,
            queue,
            kernel: Mutex::new(kernel),
            patterns,
            pattern_count,
            pattern_bytes: pattern_bytes_buffer,
            pattern_offsets: pattern_offsets_buffer,
            pattern_lengths: pattern_lengths_buffer,
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
        if self.patterns.is_empty() {
            return Vec::new();
        }

        let data_len = data.len() as cl_ulong;

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
                warn!("opencl data buffer create failed: {err}; using cpu fallback");
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
        let pattern_ids_buffer = match unsafe {
            Buffer::<cl_uint>::create(
                &self.context,
                CL_MEM_WRITE_ONLY,
                self.max_hits_per_chunk as usize,
                ptr::null_mut(),
            )
        } {
            Ok(buf) => buf,
            Err(err) => {
                warn!("opencl hit pattern buffer create failed: {err}; using cpu fallback");
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
        let patterns_mem = self.pattern_bytes.get();
        let offsets_mem = self.pattern_offsets.get();
        let lengths_mem = self.pattern_lengths.get();
        let hits_mem = hits_buffer.get();
        let pattern_ids_mem = pattern_ids_buffer.get();
        let count_mem = count_buffer.get();

        if let Err(err) = unsafe { kernel.set_arg(0, &data_mem) } {
            warn!("opencl kernel arg error: {err}; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }
        let _ = unsafe { kernel.set_arg(1, &data_len) };
        let _ = unsafe { kernel.set_arg(2, &patterns_mem) };
        let _ = unsafe { kernel.set_arg(3, &offsets_mem) };
        let _ = unsafe { kernel.set_arg(4, &lengths_mem) };
        let _ = unsafe { kernel.set_arg(5, &self.pattern_count) };
        let _ = unsafe { kernel.set_arg(6, &hits_mem) };
        let _ = unsafe { kernel.set_arg(7, &pattern_ids_mem) };
        let _ = unsafe { kernel.set_arg(8, &count_mem) };
        let _ = unsafe { kernel.set_arg(9, &self.max_hits_per_chunk) };

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
            return Vec::new();
        }

        let mut hit_offsets = vec![0u32; self.max_hits_per_chunk as usize];
        if let Err(err) = unsafe {
            self.queue.enqueue_read_buffer(
                &hits_buffer,
                CL_BLOCKING,
                0,
                &mut hit_offsets,
                &[],
            )
        } {
            warn!("opencl read hits failed: {err}; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }
        let mut hit_pattern_ids = vec![0u32; self.max_hits_per_chunk as usize];
        if let Err(err) = unsafe {
            self.queue.enqueue_read_buffer(
                &pattern_ids_buffer,
                CL_BLOCKING,
                0,
                &mut hit_pattern_ids,
                &[],
            )
        } {
            warn!("opencl read hit patterns failed: {err}; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }

        let mut hits = Vec::with_capacity(count);
        for idx in 0..count {
            let pattern_idx = hit_pattern_ids[idx] as usize;
            let Some(pattern) = self.patterns.get(pattern_idx) else {
                continue;
            };
            hits.push(Hit {
                chunk_id: chunk.id,
                local_offset: hit_offsets[idx] as u64,
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

fn build_pattern_buffers(
    patterns: &[Pattern],
) -> Result<(Vec<u8>, Vec<cl_uint>, Vec<cl_uint>)> {
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
        offsets.push(cursor as cl_uint);
        lengths.push(len as cl_uint);
        flat.extend_from_slice(&pattern.bytes);
        cursor += len as u64;
    }

    Ok((flat, offsets, lengths))
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
