use anyhow::{anyhow, Result};
use std::ptr;
use std::sync::Mutex;

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
use crate::strings::cpu::CpuStringScanner;
use crate::strings::{StringScanner, StringSpan};

const KERNEL_SRC: &str = r#"
#pragma OPENCL EXTENSION cl_khr_global_int32_base_atomics : enable
__kernel void scan_ascii_spans(
    __global const uchar* data,
    ulong data_len,
    uint min_len,
    uint max_len,
    __global uint* span_starts,
    __global uint* span_lens,
    __global uint* span_flags,
    __global uint* span_count,
    uint max_spans) {
    size_t gid = get_global_id(0);
    if (gid >= data_len) {
        return;
    }
    uchar b = data[gid];
    if (!(b == 9 || (b >= 32 && b <= 126))) {
        return;
    }
    if (gid > 0) {
        uchar prev = data[gid - 1];
        if (prev == 9 || (prev >= 32 && prev <= 126)) {
            return;
        }
    }

    uint len = 0;
    uint digits = 0;
    uint flags = 0;
    uint window = 0;
    const uint HTTP = 0x68747470;
    const uint WWW = 0x7777772e;

    while ((ulong)(gid + len) < data_len) {
        uchar c = data[gid + len];
        if (!(c == 9 || (c >= 32 && c <= 126))) {
            break;
        }
        uchar lower = c;
        if (lower >= 'A' && lower <= 'Z') {
            lower = (uchar)(lower + 32);
        }
        window = (window << 8) | (uint)lower;
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
        uint idx = atomic_inc(span_count);
        if (idx < max_spans) {
            span_starts[idx] = (uint)gid;
            span_lens[idx] = len;
            span_flags[idx] = flags;
        }
    }
}
"#;

pub struct OpenClStringScanner {
    context: Context,
    queue: CommandQueue,
    kernel: Mutex<Kernel>,
    min_len: usize,
    max_len: usize,
    min_len_u32: u32,
    max_len_u32: u32,
    max_spans_per_chunk: u32,
    scan_utf16: bool,
    cpu_fallback: CpuStringScanner,
}

impl OpenClStringScanner {
    pub fn new(cfg: &Config) -> Result<Self> {
        let (_device, context) = select_device(cfg)?;
        #[allow(deprecated)]
        let queue = CommandQueue::create_default(&context, 0)?;
        let program = Program::create_and_build_from_source(&context, KERNEL_SRC, "")
            .map_err(|err| anyhow!(err))?;
        let kernel = Kernel::create(&program, "scan_ascii_spans")?;
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
            context,
            queue,
            kernel: Mutex::new(kernel),
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

impl StringScanner for OpenClStringScanner {
    fn scan_chunk(&self, chunk: &ScanChunk, data: &[u8]) -> Vec<StringSpan> {
        if data.is_empty() {
            return Vec::new();
        }
        if data.len() > u32::MAX as usize {
            warn!("chunk length exceeds u32::MAX; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
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

        let span_capacity = self.max_spans_per_chunk as usize;
        let starts_buffer = match unsafe {
            Buffer::<cl_uint>::create(
                &self.context,
                CL_MEM_WRITE_ONLY,
                span_capacity,
                ptr::null_mut(),
            )
        } {
            Ok(buf) => buf,
            Err(err) => {
                warn!("opencl span starts buffer create failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };
        let lens_buffer = match unsafe {
            Buffer::<cl_uint>::create(
                &self.context,
                CL_MEM_WRITE_ONLY,
                span_capacity,
                ptr::null_mut(),
            )
        } {
            Ok(buf) => buf,
            Err(err) => {
                warn!("opencl span lengths buffer create failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };
        let flags_buffer = match unsafe {
            Buffer::<cl_uint>::create(
                &self.context,
                CL_MEM_WRITE_ONLY,
                span_capacity,
                ptr::null_mut(),
            )
        } {
            Ok(buf) => buf,
            Err(err) => {
                warn!("opencl span flags buffer create failed: {err}; using cpu fallback");
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
                warn!("opencl span count buffer create failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };

        let kernel = match self.kernel.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        let data_mem = data_buffer.get();
        let starts_mem = starts_buffer.get();
        let lens_mem = lens_buffer.get();
        let flags_mem = flags_buffer.get();
        let count_mem = count_buffer.get();

        if let Err(err) = unsafe { kernel.set_arg(0, &data_mem) } {
            warn!("opencl kernel arg error: {err}; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }
        let _ = unsafe { kernel.set_arg(1, &data_len) };
        let _ = unsafe { kernel.set_arg(2, &self.min_len_u32) };
        let _ = unsafe { kernel.set_arg(3, &self.max_len_u32) };
        let _ = unsafe { kernel.set_arg(4, &starts_mem) };
        let _ = unsafe { kernel.set_arg(5, &lens_mem) };
        let _ = unsafe { kernel.set_arg(6, &flags_mem) };
        let _ = unsafe { kernel.set_arg(7, &count_mem) };
        let _ = unsafe { kernel.set_arg(8, &self.max_spans_per_chunk) };

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

        let mut count = [0u32];
        if let Err(err) = unsafe {
            self.queue
                .enqueue_read_buffer(&count_buffer, CL_BLOCKING, 0, &mut count, &[])
        } {
            warn!("opencl read span count failed: {err}; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }
        let mut count = count[0] as usize;
        if count > span_capacity {
            warn!(
                "opencl span overflow: count={} max={}",
                count, span_capacity
            );
            count = span_capacity;
        }

        let mut starts = vec![0u32; span_capacity];
        let mut lens = vec![0u32; span_capacity];
        let mut flags = vec![0u32; span_capacity];
        if let Err(err) = unsafe {
            self.queue
                .enqueue_read_buffer(&starts_buffer, CL_BLOCKING, 0, &mut starts, &[])
        } {
            warn!("opencl read span starts failed: {err}; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }
        if let Err(err) = unsafe {
            self.queue
                .enqueue_read_buffer(&lens_buffer, CL_BLOCKING, 0, &mut lens, &[])
        } {
            warn!("opencl read span lengths failed: {err}; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }
        if let Err(err) = unsafe {
            self.queue
                .enqueue_read_buffer(&flags_buffer, CL_BLOCKING, 0, &mut flags, &[])
        } {
            warn!("opencl read span flags failed: {err}; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }

        let mut spans = Vec::with_capacity(count);
        for idx in 0..count {
            spans.push(StringSpan {
                chunk_id: chunk.id,
                local_start: starts[idx] as u64,
                length: lens[idx],
                flags: flags[idx],
            });
        }
        spans = extend_long_ascii_spans(chunk, data, spans, self.min_len, self.max_len);
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

fn select_device(cfg: &Config) -> Result<(Device, Context)> {
    let platforms = get_platforms()?;
    if platforms.is_empty() {
        return Err(anyhow!("no OpenCL platforms available"));
    }

    if let (Some(platform_idx), Some(device_idx)) = (cfg.opencl_platform_index, cfg.opencl_device_index)
    {
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
