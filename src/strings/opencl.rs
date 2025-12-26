use anyhow::{anyhow, Result};
use std::ptr;
use std::sync::Mutex;

use opencl3::command_queue::{CommandQueue, CL_BLOCKING};
use opencl3::context::Context;
use opencl3::device::{Device, CL_DEVICE_TYPE_GPU};
use opencl3::kernel::Kernel;
use opencl3::memory::{Buffer, ClMem, CL_MEM_COPY_HOST_PTR, CL_MEM_READ_ONLY, CL_MEM_WRITE_ONLY};
use opencl3::platform::get_platforms;
use opencl3::program::Program;
use opencl3::types::cl_ulong;
use tracing::warn;

use crate::chunk::ScanChunk;
use crate::config::Config;
use crate::strings::cpu::CpuStringScanner;
use crate::strings::{StringScanner, StringSpan};

const KERNEL_SRC: &str = r#"
__kernel void mark_printable(
    __global const uchar* data,
    ulong data_len,
    __global uchar* mask) {
    size_t gid = get_global_id(0);
    if (gid >= data_len) {
        return;
    }
    uchar b = data[gid];
    if (b == 9 || (b >= 32 && b <= 126)) {
        mask[gid] = 1;
    } else {
        mask[gid] = 0;
    }
}
"#;

pub struct OpenClStringScanner {
    context: Context,
    queue: CommandQueue,
    kernel: Mutex<Kernel>,
    min_len: usize,
    max_len: usize,
    cpu_fallback: CpuStringScanner,
}

impl OpenClStringScanner {
    pub fn new(cfg: &Config) -> Result<Self> {
        let (_device, context) = select_device(cfg)?;
        #[allow(deprecated)]
        let queue = CommandQueue::create_default(&context, 0)?;
        let program = Program::create_and_build_from_source(&context, KERNEL_SRC, "")
            .map_err(|err| anyhow!(err))?;
        let kernel = Kernel::create(&program, "mark_printable")?;
        let max_len = if cfg.string_max_len == 0 {
            usize::MAX
        } else {
            cfg.string_max_len
        };

        Ok(Self {
            context,
            queue,
            kernel: Mutex::new(kernel),
            min_len: cfg.string_min_len,
            max_len,
            cpu_fallback: CpuStringScanner::new(cfg.string_min_len, cfg.string_max_len),
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

        let mask_buffer = match unsafe {
            Buffer::<u8>::create(
                &self.context,
                CL_MEM_WRITE_ONLY,
                data.len(),
                ptr::null_mut(),
            )
        } {
            Ok(buf) => buf,
            Err(err) => {
                warn!("opencl mask buffer create failed: {err}; using cpu fallback");
                return self.cpu_fallback.scan_chunk(chunk, data);
            }
        };

        let kernel = match self.kernel.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        let data_mem = data_buffer.get();
        let mask_mem = mask_buffer.get();

        if let Err(err) = unsafe { kernel.set_arg(0, &data_mem) } {
            warn!("opencl kernel arg error: {err}; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }
        let _ = unsafe { kernel.set_arg(1, &data_len) };
        let _ = unsafe { kernel.set_arg(2, &mask_mem) };

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

        let mut mask = vec![0u8; data.len()];
        if let Err(err) = unsafe {
            self.queue
                .enqueue_read_buffer(&mask_buffer, CL_BLOCKING, 0, &mut mask, &[])
        } {
            warn!("opencl read mask failed: {err}; using cpu fallback");
            return self.cpu_fallback.scan_chunk(chunk, data);
        }

        spans_from_mask(chunk, &mask, self.min_len, self.max_len)
    }
}

fn spans_from_mask(
    chunk: &ScanChunk,
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
            spans.push(StringSpan {
                chunk_id: chunk.id,
                local_start: start as u64,
                length: len as u32,
                flags: 0,
            });
        }

        if len >= max_len {
            continue;
        }
    }

    spans
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
