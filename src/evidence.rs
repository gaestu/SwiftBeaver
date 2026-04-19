use std::fs::{File, OpenOptions};
use std::sync::Arc;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvidenceError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("internal evidence error: {0}")]
    Internal(String),
    #[error("unsupported evidence type: {0}")]
    Unsupported(String),
    #[error("invalid evidence offset: {0}")]
    InvalidOffset(String),
}

/// A read-only evidence source backed by a linear byte space.
///
/// # Example
/// ```rust
/// use swiftbeaver::evidence::{EvidenceSource, RawFileSource};
///
/// let path = std::env::temp_dir().join("SwiftBeaver_evidence_example.bin");
/// std::fs::write(&path, b"hello").unwrap();
/// let source = RawFileSource::open(&path).unwrap();
/// let mut buf = [0u8; 5];
/// source.read_at(0, &mut buf).unwrap();
/// assert_eq!(&buf, b"hello");
/// ```
pub trait EvidenceSource: Send + Sync {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError>;
}

pub struct RawFileSource {
    file: File,
    len: u64,
    #[cfg(not(unix))]
    lock: std::sync::Mutex<()>,
}

impl RawFileSource {
    pub fn open(path: &std::path::Path) -> Result<Self, EvidenceError> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            file,
            len,
            #[cfg(not(unix))]
            lock: std::sync::Mutex::new(()),
        })
    }
}

impl EvidenceSource for RawFileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            Ok(self.file.read_at(buf, offset)?)
        }
        #[cfg(not(unix))]
        {
            use std::io::{Read, Seek, SeekFrom};
            let _guard = self
                .lock
                .lock()
                .map_err(|_| EvidenceError::Internal("raw file lock poisoned".to_string()))?;
            let mut f = &self.file;
            f.seek(SeekFrom::Start(offset))?;
            Ok(f.read(buf)?)
        }
    }
}

pub struct DeviceSource {
    file: File,
    len: u64,
    #[cfg(not(unix))]
    lock: std::sync::Mutex<()>,
}

impl DeviceSource {
    pub fn open(path: &std::path::Path) -> Result<Self, EvidenceError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;

            let metadata = path.metadata()?;
            if !metadata.file_type().is_block_device() {
                return Err(EvidenceError::Unsupported(
                    "path is not a block device".to_string(),
                ));
            }

            let file = OpenOptions::new().read(true).open(path)?;
            let len = device_len(&file, metadata.len())?;
            Ok(Self {
                file,
                len,
                #[cfg(not(unix))]
                lock: std::sync::Mutex::new(()),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(EvidenceError::Unsupported(
                "device input is only supported on unix".to_string(),
            ))
        }
    }
}

impl EvidenceSource for DeviceSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            Ok(self.file.read_at(buf, offset)?)
        }
        #[cfg(not(unix))]
        {
            use std::io::{Read, Seek, SeekFrom};
            let _guard = self
                .lock
                .lock()
                .map_err(|_| EvidenceError::Internal("device lock poisoned".to_string()))?;
            let mut f = &self.file;
            f.seek(SeekFrom::Start(offset))?;
            Ok(f.read(buf)?)
        }
    }
}

#[cfg(target_os = "linux")]
fn device_len(file: &File, fallback_len: u64) -> Result<u64, EvidenceError> {
    use std::os::unix::io::AsRawFd;

    const BLKGETSIZE64: libc::c_ulong = 0x80081272;
    let mut size: u64 = 0;
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), BLKGETSIZE64, &mut size) };
    if rc != 0 {
        if fallback_len > 0 {
            return Ok(fallback_len);
        }
        return Err(EvidenceError::Unsupported(
            "unable to read block device size".to_string(),
        ));
    }
    Ok(size)
}

#[cfg(not(target_os = "linux"))]
fn device_len(_file: &File, fallback_len: u64) -> Result<u64, EvidenceError> {
    Ok(fallback_len)
}

#[cfg(feature = "ewf")]
mod ewf {
    use std::ffi::{CStr, CString};
    use std::path::Path;
    use std::ptr;
    use std::sync::Mutex;

    use libc::{c_char, c_int, c_void, off64_t, size_t, ssize_t};

    use super::{EvidenceError, EvidenceSource};

    type LibEwfHandle = libc::intptr_t;
    type LibEwfError = libc::intptr_t;

    const LIBEWF_FORMAT_UNKNOWN: u8 = 0x00;

    #[link(name = "ewf")]
    unsafe extern "C" {
        fn libewf_get_access_flags_read() -> c_int;

        fn libewf_check_file_signature(
            filename: *const c_char,
            error: *mut *mut LibEwfError,
        ) -> c_int;
        fn libewf_glob(
            filename: *const c_char,
            filename_length: size_t,
            format: u8,
            filenames: *mut *mut *mut c_char,
            number_of_filenames: *mut c_int,
            error: *mut *mut LibEwfError,
        ) -> c_int;
        fn libewf_glob_free(
            filenames: *mut *mut c_char,
            number_of_filenames: c_int,
            error: *mut *mut LibEwfError,
        ) -> c_int;

        fn libewf_handle_initialize(
            handle: *mut *mut LibEwfHandle,
            error: *mut *mut LibEwfError,
        ) -> c_int;
        fn libewf_handle_free(
            handle: *mut *mut LibEwfHandle,
            error: *mut *mut LibEwfError,
        ) -> c_int;
        fn libewf_handle_open(
            handle: *mut LibEwfHandle,
            filenames: *mut *mut c_char,
            number_of_filenames: c_int,
            access_flags: c_int,
            error: *mut *mut LibEwfError,
        ) -> c_int;
        fn libewf_handle_close(handle: *mut LibEwfHandle, error: *mut *mut LibEwfError) -> c_int;
        fn libewf_handle_get_media_size(
            handle: *mut LibEwfHandle,
            media_size: *mut u64,
            error: *mut *mut LibEwfError,
        ) -> c_int;
        fn libewf_handle_read_random(
            handle: *mut LibEwfHandle,
            buffer: *mut c_void,
            buffer_size: size_t,
            offset: off64_t,
            error: *mut *mut LibEwfError,
        ) -> ssize_t;

        fn libewf_error_sprint(error: *mut LibEwfError, string: *mut c_char, size: size_t)
        -> c_int;
        fn libewf_error_free(error: *mut *mut LibEwfError);
    }

    struct HandleInner {
        handle: *mut LibEwfHandle,
    }

    pub struct EwfSource {
        handle: Mutex<HandleInner>,
        len: u64,
    }

    // SAFETY: libewf handle access is serialized via the mutex.
    unsafe impl Send for EwfSource {}
    unsafe impl Sync for EwfSource {}

    impl EwfSource {
        pub fn open(path: &Path) -> Result<Self, EvidenceError> {
            let c_path = CString::new(path.to_string_lossy().as_bytes())
                .map_err(|_| EvidenceError::Unsupported("path contains null byte".to_string()))?;

            unsafe {
                let mut error: *mut LibEwfError = ptr::null_mut();
                let sig = libewf_check_file_signature(c_path.as_ptr(), &mut error);
                if sig <= 0 {
                    let msg = if sig == 0 {
                        "not an EWF image".to_string()
                    } else {
                        error_to_string(error)
                    };
                    return Err(EvidenceError::Unsupported(msg));
                }
            }

            unsafe {
                let mut error: *mut LibEwfError = ptr::null_mut();
                let mut handle: *mut LibEwfHandle = ptr::null_mut();
                if libewf_handle_initialize(&mut handle, &mut error) != 1 {
                    return Err(EvidenceError::Unsupported(error_to_string(error)));
                }

                let mut filenames: *mut *mut c_char = ptr::null_mut();
                let mut number_of_filenames: c_int = 0;
                let rc = libewf_glob(
                    c_path.as_ptr(),
                    c_path.as_bytes().len(),
                    LIBEWF_FORMAT_UNKNOWN,
                    &mut filenames,
                    &mut number_of_filenames,
                    &mut error,
                );
                if rc != 1 {
                    let _ = libewf_handle_free(&mut handle, &mut error);
                    return Err(EvidenceError::Unsupported(error_to_string(error)));
                }

                let access_flags = libewf_get_access_flags_read();
                let rc = libewf_handle_open(
                    handle,
                    filenames,
                    number_of_filenames,
                    access_flags,
                    &mut error,
                );
                let _ = libewf_glob_free(filenames, number_of_filenames, &mut error);
                if rc != 1 {
                    let _ = libewf_handle_free(&mut handle, &mut error);
                    return Err(EvidenceError::Unsupported(error_to_string(error)));
                }

                let mut media_size: u64 = 0;
                if libewf_handle_get_media_size(handle, &mut media_size, &mut error) != 1 {
                    let _ = libewf_handle_close(handle, &mut error);
                    let _ = libewf_handle_free(&mut handle, &mut error);
                    return Err(EvidenceError::Unsupported(error_to_string(error)));
                }

                Ok(Self {
                    handle: Mutex::new(HandleInner { handle }),
                    len: media_size,
                })
            }
        }
    }

    impl EvidenceSource for EwfSource {
        fn len(&self) -> u64 {
            self.len
        }

        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError> {
            if offset > i64::MAX as u64 {
                return Err(EvidenceError::InvalidOffset(format!(
                    "offset too large for libewf: {offset}"
                )));
            }

            let guard = self
                .handle
                .lock()
                .map_err(|_| EvidenceError::Internal("libewf handle lock poisoned".to_string()))?;
            if guard.handle.is_null() {
                return Err(EvidenceError::Unsupported(
                    "libewf handle closed".to_string(),
                ));
            }

            unsafe {
                let mut error: *mut LibEwfError = ptr::null_mut();
                let read = libewf_handle_read_random(
                    guard.handle,
                    buf.as_mut_ptr() as *mut c_void,
                    buf.len(),
                    offset as off64_t,
                    &mut error,
                );
                if read < 0 {
                    return Err(EvidenceError::Unsupported(error_to_string(error)));
                }
                Ok(read as usize)
            }
        }
    }

    impl Drop for EwfSource {
        fn drop(&mut self) {
            if let Ok(mut guard) = self.handle.lock() {
                if guard.handle.is_null() {
                    return;
                }
                unsafe {
                    let mut error: *mut LibEwfError = ptr::null_mut();
                    let _ = libewf_handle_close(guard.handle, &mut error);
                    if !error.is_null() {
                        libewf_error_free(&mut error);
                    }
                    let mut handle = guard.handle;
                    let _ = libewf_handle_free(&mut handle, &mut error);
                    if !error.is_null() {
                        libewf_error_free(&mut error);
                    }
                }
                guard.handle = ptr::null_mut();
            }
        }
    }

    pub struct PooledEwfSource {
        handles: Vec<Mutex<HandleInner>>,
        len: u64,
        next: std::sync::atomic::AtomicUsize,
    }

    // SAFETY: All handle access is serialized via individual Mutexes.
    unsafe impl Send for PooledEwfSource {}
    unsafe impl Sync for PooledEwfSource {}

    /// Close and free a list of already-opened handles, then free the globbed
    /// filenames array.  Used to clean up on error during pool construction.
    unsafe fn cleanup_handles_and_glob(
        handles: &[Mutex<HandleInner>],
        filenames: *mut *mut c_char,
        number_of_filenames: c_int,
    ) {
        for h in handles {
            let guard = match Mutex::lock(h) {
                Ok(g) => g,
                Err(_) => continue,
            };
            if !guard.handle.is_null() {
                unsafe {
                    let mut err: *mut LibEwfError = ptr::null_mut();
                    let _ = libewf_handle_close(guard.handle, &mut err);
                    if !err.is_null() {
                        libewf_error_free(&mut err);
                    }
                    let mut h_ptr = guard.handle;
                    let _ = libewf_handle_free(&mut h_ptr, &mut err);
                    if !err.is_null() {
                        libewf_error_free(&mut err);
                    }
                }
            }
        }
        unsafe {
            let mut err: *mut LibEwfError = ptr::null_mut();
            let _ = libewf_glob_free(filenames, number_of_filenames, &mut err);
            if !err.is_null() {
                libewf_error_free(&mut err);
            }
        }
    }

    impl PooledEwfSource {
        pub fn open(path: &Path, count: usize) -> Result<Self, EvidenceError> {
            use tracing::info;

            if count == 0 {
                return Err(EvidenceError::Unsupported(
                    "EWF pool count must be at least 1".to_string(),
                ));
            }

            let c_path = CString::new(path.to_string_lossy().as_bytes())
                .map_err(|_| EvidenceError::Unsupported("path contains null byte".to_string()))?;

            // Verify EWF signature once
            unsafe {
                let mut error: *mut LibEwfError = ptr::null_mut();
                let sig = libewf_check_file_signature(c_path.as_ptr(), &mut error);
                if sig <= 0 {
                    let msg = if sig == 0 {
                        "not an EWF image".to_string()
                    } else {
                        error_to_string(error)
                    };
                    return Err(EvidenceError::Unsupported(msg));
                }
            }

            // Glob filenames once (shared across all handles)
            let (filenames, number_of_filenames) = unsafe {
                let mut error: *mut LibEwfError = ptr::null_mut();
                let mut filenames: *mut *mut c_char = ptr::null_mut();
                let mut number_of_filenames: c_int = 0;
                let rc = libewf_glob(
                    c_path.as_ptr(),
                    c_path.as_bytes().len(),
                    LIBEWF_FORMAT_UNKNOWN,
                    &mut filenames,
                    &mut number_of_filenames,
                    &mut error,
                );
                if rc != 1 {
                    return Err(EvidenceError::Unsupported(error_to_string(error)));
                }
                (filenames, number_of_filenames)
            };

            let access_flags = unsafe { libewf_get_access_flags_read() };
            let mut handles = Vec::with_capacity(count);
            let mut first_media_size: Option<u64> = None;

            for i in 0..count {
                unsafe {
                    let mut error: *mut LibEwfError = ptr::null_mut();
                    let mut handle: *mut LibEwfHandle = ptr::null_mut();

                    if libewf_handle_initialize(&mut handle, &mut error) != 1 {
                        let msg = error_to_string(error);
                        cleanup_handles_and_glob(&handles, filenames, number_of_filenames);
                        return Err(EvidenceError::Unsupported(msg));
                    }

                    let rc = libewf_handle_open(
                        handle,
                        filenames,
                        number_of_filenames,
                        access_flags,
                        &mut error,
                    );
                    if rc != 1 {
                        let msg = error_to_string(error);
                        let mut err2: *mut LibEwfError = ptr::null_mut();
                        let _ = libewf_handle_free(&mut handle, &mut err2);
                        if !err2.is_null() {
                            libewf_error_free(&mut err2);
                        }
                        cleanup_handles_and_glob(&handles, filenames, number_of_filenames);
                        return Err(EvidenceError::Unsupported(msg));
                    }

                    let mut media_size: u64 = 0;
                    if libewf_handle_get_media_size(handle, &mut media_size, &mut error) != 1 {
                        let msg = error_to_string(error);
                        let mut err2: *mut LibEwfError = ptr::null_mut();
                        let _ = libewf_handle_close(handle, &mut err2);
                        if !err2.is_null() {
                            libewf_error_free(&mut err2);
                        }
                        let _ = libewf_handle_free(&mut handle, &mut err2);
                        if !err2.is_null() {
                            libewf_error_free(&mut err2);
                        }
                        cleanup_handles_and_glob(&handles, filenames, number_of_filenames);
                        return Err(EvidenceError::Unsupported(msg));
                    }

                    match first_media_size {
                        None => first_media_size = Some(media_size),
                        Some(expected) if expected != media_size => {
                            let mut err2: *mut LibEwfError = ptr::null_mut();
                            let _ = libewf_handle_close(handle, &mut err2);
                            if !err2.is_null() {
                                libewf_error_free(&mut err2);
                            }
                            let _ = libewf_handle_free(&mut handle, &mut err2);
                            if !err2.is_null() {
                                libewf_error_free(&mut err2);
                            }
                            cleanup_handles_and_glob(&handles, filenames, number_of_filenames);
                            return Err(EvidenceError::Unsupported(format!(
                                "EWF handle {i} reports media_size {media_size}, expected {expected}"
                            )));
                        }
                        _ => {}
                    }

                    handles.push(Mutex::new(HandleInner { handle }));
                }
            }

            // Free the globbed filenames array
            unsafe {
                let mut error: *mut LibEwfError = ptr::null_mut();
                let _ = libewf_glob_free(filenames, number_of_filenames, &mut error);
                if !error.is_null() {
                    libewf_error_free(&mut error);
                }
            }

            // first_media_size is guaranteed Some because count >= 1 and
            // the loop would have returned Err before reaching this point
            // if any handle failed to open or report its media size.
            let len = first_media_size.expect("at least one handle opened successfully");
            info!("opened {} EWF reader handles", count);

            Ok(Self {
                handles,
                len,
                next: std::sync::atomic::AtomicUsize::new(0),
            })
        }
    }

    impl EvidenceSource for PooledEwfSource {
        fn len(&self) -> u64 {
            self.len
        }

        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError> {
            use std::sync::atomic::Ordering;

            if offset > i64::MAX as u64 {
                return Err(EvidenceError::InvalidOffset(format!(
                    "offset too large for libewf: {offset}"
                )));
            }

            let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.handles.len();
            let guard = self.handles[idx].lock().map_err(|_| {
                EvidenceError::Internal("libewf pool handle lock poisoned".to_string())
            })?;
            if guard.handle.is_null() {
                return Err(EvidenceError::Unsupported(
                    "libewf handle closed".to_string(),
                ));
            }

            unsafe {
                let mut error: *mut LibEwfError = ptr::null_mut();
                let read = libewf_handle_read_random(
                    guard.handle,
                    buf.as_mut_ptr() as *mut c_void,
                    buf.len(),
                    offset as off64_t,
                    &mut error,
                );
                if read < 0 {
                    return Err(EvidenceError::Unsupported(error_to_string(error)));
                }
                Ok(read as usize)
            }
        }
    }

    impl Drop for PooledEwfSource {
        fn drop(&mut self) {
            for mutex in &self.handles {
                if let Ok(mut guard) = mutex.lock() {
                    if guard.handle.is_null() {
                        continue;
                    }
                    unsafe {
                        let mut error: *mut LibEwfError = ptr::null_mut();
                        let _ = libewf_handle_close(guard.handle, &mut error);
                        if !error.is_null() {
                            libewf_error_free(&mut error);
                        }
                        let mut handle = guard.handle;
                        let _ = libewf_handle_free(&mut handle, &mut error);
                        if !error.is_null() {
                            libewf_error_free(&mut error);
                        }
                    }
                    guard.handle = ptr::null_mut();
                }
            }
        }
    }

    unsafe fn error_to_string(mut error: *mut LibEwfError) -> String {
        if error.is_null() {
            return "libewf error".to_string();
        }

        let mut buf = vec![0i8; 1024];
        let rc = unsafe { libewf_error_sprint(error, buf.as_mut_ptr(), buf.len()) };
        let msg = if rc >= 0 {
            unsafe { CStr::from_ptr(buf.as_ptr()) }
                .to_string_lossy()
                .trim_end_matches('\0')
                .to_string()
        } else {
            "libewf error".to_string()
        };
        unsafe {
            libewf_error_free(&mut error);
        }
        msg
    }
}

use crate::cli::CliOptions;

pub fn open_source(
    opts: &CliOptions,
    ewf_reader_handles: usize,
) -> Result<Box<dyn EvidenceSource>, EvidenceError> {
    if is_ewf_path(&opts.input) {
        #[cfg(feature = "ewf")]
        {
            if ewf_reader_handles > 1 {
                let src = ewf::PooledEwfSource::open(&opts.input, ewf_reader_handles)?;
                return Ok(Box::new(src));
            }
            let src = ewf::EwfSource::open(&opts.input)?;
            return Ok(Box::new(src));
        }
        #[cfg(not(feature = "ewf"))]
        {
            let _ = ewf_reader_handles;
            return Err(EvidenceError::Unsupported(
                "E01 support requires the `ewf` feature and libewf".to_string(),
            ));
        }
    }

    if is_block_device(&opts.input)? {
        let src = DeviceSource::open(&opts.input)?;
        return Ok(Box::new(src));
    }

    let src = RawFileSource::open(&opts.input)?;
    Ok(Box::new(src))
}

/// Return true when the input path should be opened as an EWF evidence source.
pub fn is_ewf_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("e01"))
        .unwrap_or(false)
}

fn is_block_device(path: &std::path::Path) -> Result<bool, EvidenceError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        let metadata = std::fs::metadata(path)?;
        Ok(metadata.file_type().is_block_device())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(false)
    }
}

pub fn compute_sha256(
    evidence: &dyn EvidenceSource,
    chunk_size: usize,
) -> Result<String, EvidenceError> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    let total_len = evidence.len();
    let mut offset = 0u64;
    let mut buf = vec![0u8; chunk_size.max(1)];

    while offset < total_len {
        let remaining = total_len - offset;
        let read_len = remaining.min(buf.len() as u64) as usize;
        let n = evidence.read_at(offset, &mut buf[..read_len])?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        offset = offset.saturating_add(n as u64);
    }

    Ok(hex::encode(hasher.finalize()))
}

/// Read-through LRU segment cache for any `EvidenceSource`.
///
/// Each cached segment is aligned to `segment_size` boundaries and stores up
/// to `segment_size` bytes. The final segment may be shorter at EOF.
/// Thread-safe: cache access is sharded across independent `Mutex` instances,
/// and inner source reads are performed WITHOUT holding any shard lock.
pub struct CachedEwfSource {
    inner: Box<dyn EvidenceSource>,
    shards: Vec<std::sync::Mutex<lru::LruCache<u64, Arc<Vec<u8>>>>>,
    segment_size: usize,
}

impl CachedEwfSource {
    /// Create a sharded segment cache around an evidence source.
    ///
    /// Uses the default shard count and keeps the total cache capacity bounded
    /// by `segments` across all shards. A `segments` value of `0` is clamped
    /// to a single cache slot.
    pub fn new(inner: Box<dyn EvidenceSource>, segments: usize, segment_size: usize) -> Self {
        Self::with_shards(
            inner,
            segments,
            segment_size,
            crate::constants::DEFAULT_CACHE_SHARDS,
        )
    }

    fn with_shards(
        inner: Box<dyn EvidenceSource>,
        segments: usize,
        segment_size: usize,
        shard_count: usize,
    ) -> Self {
        let total_segments = segments.max(1);
        let segment_size = segment_size.max(1);
        let shard_count = shard_count.max(1).min(total_segments);
        let mut shards = Vec::with_capacity(shard_count);
        let base_capacity = total_segments / shard_count;
        let extra = total_segments % shard_count;

        for shard_idx in 0..shard_count {
            let shard_capacity = base_capacity + usize::from(shard_idx < extra);
            let cap = std::num::NonZeroUsize::new(shard_capacity.max(1))
                .unwrap_or(std::num::NonZeroUsize::MIN);
            shards.push(std::sync::Mutex::new(lru::LruCache::new(cap)));
        }

        Self {
            inner,
            shards,
            segment_size,
        }
    }

    fn shard_index(&self, segment_start: u64) -> usize {
        let segment_number = segment_start / self.segment_size as u64;
        (segment_number % self.shards.len() as u64) as usize
    }

    fn cached_segment(&self, segment_start: u64) -> Result<Option<Arc<Vec<u8>>>, EvidenceError> {
        let shard_idx = self.shard_index(segment_start);
        let mut cache = self.shards[shard_idx]
            .lock()
            .map_err(|_| EvidenceError::Internal("segment cache lock poisoned".to_string()))?;
        Ok(cache.get(&segment_start).map(Arc::clone))
    }

    fn insert_segment(
        &self,
        segment_start: u64,
        segment_data: Arc<Vec<u8>>,
    ) -> Result<Arc<Vec<u8>>, EvidenceError> {
        let shard_idx = self.shard_index(segment_start);
        let mut cache = self.shards[shard_idx]
            .lock()
            .map_err(|_| EvidenceError::Internal("segment cache lock poisoned".to_string()))?;

        if let Some(existing) = cache.get(&segment_start) {
            return Ok(Arc::clone(existing));
        }

        cache.put(segment_start, Arc::clone(&segment_data));
        Ok(segment_data)
    }

    fn read_segment(&self, segment_start: u64) -> Result<Vec<u8>, EvidenceError> {
        let total = self.inner.len();
        let remaining = total.saturating_sub(segment_start);
        let read_len = (self.segment_size as u64).min(remaining) as usize;
        if read_len == 0 {
            return Ok(Vec::new());
        }
        let mut buf = vec![0u8; read_len];
        let mut filled = 0usize;
        while filled < read_len {
            let n = self
                .inner
                .read_at(segment_start + filled as u64, &mut buf[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        buf.truncate(filled);
        Ok(buf)
    }
}

impl EvidenceSource for CachedEwfSource {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError> {
        if buf.is_empty() {
            return Ok(0);
        }

        let seg_size = self.segment_size as u64;
        let mut dst_offset = 0usize;
        let mut cur_offset = offset;

        while dst_offset < buf.len() {
            let segment_start = (cur_offset / seg_size) * seg_size;
            let offset_in_segment = (cur_offset - segment_start) as usize;

            let segment_data = if let Some(data) = self.cached_segment(segment_start)? {
                data
            } else {
                // Read from inner source WITHOUT holding any shard lock.
                let data = Arc::new(self.read_segment(segment_start)?);
                self.insert_segment(segment_start, data)?
            };

            if offset_in_segment >= segment_data.len() {
                break; // past end of evidence
            }

            let available = segment_data.len() - offset_in_segment;
            let needed = buf.len() - dst_offset;
            let copy_len = available.min(needed);
            buf[dst_offset..dst_offset + copy_len]
                .copy_from_slice(&segment_data[offset_in_segment..offset_in_segment + copy_len]);
            dst_offset += copy_len;
            cur_offset += copy_len as u64;
        }

        Ok(dst_offset)
    }
}

/// Wrap an evidence source with a sharded LRU segment cache.
///
/// Uses `DEFAULT_CACHE_SHARDS` shard partitions and the default I/O buffer size
/// as the segment size. If `segments` is 0, returns the source unwrapped.
pub fn wrap_with_cache(
    source: Box<dyn EvidenceSource>,
    segments: usize,
) -> Box<dyn EvidenceSource> {
    if segments == 0 {
        return source;
    }
    Box::new(CachedEwfSource::new(
        source,
        segments,
        crate::constants::DEFAULT_IO_BUFFER_SIZE,
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{
        CachedEwfSource, EvidenceError, EvidenceSource, RawFileSource, compute_sha256, is_ewf_path,
    };

    struct TestEvidenceSource {
        data: Vec<u8>,
        read_calls: AtomicUsize,
    }

    impl TestEvidenceSource {
        fn new(data: Vec<u8>) -> Self {
            Self {
                data,
                read_calls: AtomicUsize::new(0),
            }
        }

        fn read_calls(&self) -> usize {
            self.read_calls.load(Ordering::SeqCst)
        }
    }

    impl EvidenceSource for Arc<TestEvidenceSource> {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }

        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError> {
            self.read_calls.fetch_add(1, Ordering::SeqCst);
            let start = offset as usize;
            if start >= self.data.len() {
                return Ok(0);
            }
            let available = self.data.len() - start;
            let copy_len = available.min(buf.len());
            buf[..copy_len].copy_from_slice(&self.data[start..start + copy_len]);
            Ok(copy_len)
        }
    }

    #[test]
    fn ewf_extension_detection() {
        assert!(is_ewf_path(std::path::Path::new("case.E01")));
        assert!(is_ewf_path(std::path::Path::new("case.e01")));
        assert!(!is_ewf_path(std::path::Path::new("case.dd")));
    }

    #[test]
    fn computes_sha256_for_raw_file() {
        use std::fs;

        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("image.bin");
        fs::write(&path, b"abc").expect("write");

        let src = RawFileSource::open(&path).expect("open");
        let hash = compute_sha256(&src, 4).expect("hash");
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn cached_ewf_source_reuses_segment_on_cache_hit() {
        let backing = Arc::new(TestEvidenceSource::new((0u8..32).collect()));
        let cached = CachedEwfSource::with_shards(Box::new(Arc::clone(&backing)), 4, 8, 4);
        let mut first = [0u8; 4];
        let mut second = [0u8; 4];

        let n1 = cached.read_at(2, &mut first).expect("first read");
        let n2 = cached.read_at(4, &mut second).expect("second read");

        assert_eq!(n1, 4);
        assert_eq!(n2, 4);
        assert_eq!(&first, &[2, 3, 4, 5]);
        assert_eq!(&second, &[4, 5, 6, 7]);
        assert_eq!(backing.read_calls(), 1, "expected a single backing read");
    }

    #[test]
    fn cached_ewf_source_reads_across_segment_boundaries() {
        let data: Vec<u8> = (0u8..24).collect();
        let cached = CachedEwfSource::with_shards(
            Box::new(Arc::new(TestEvidenceSource::new(data.clone()))),
            4,
            8,
            4,
        );
        let mut buf = [0u8; 10];

        let n = cached.read_at(6, &mut buf).expect("cross-segment read");

        assert_eq!(n, 10);
        assert_eq!(&buf, &data[6..16]);
    }

    #[test]
    fn cached_ewf_source_normalizes_zero_segment_size() {
        let cached = CachedEwfSource::with_shards(
            Box::new(Arc::new(TestEvidenceSource::new((0u8..8).collect()))),
            4,
            0,
            4,
        );
        let mut buf = [0u8; 3];

        let n = cached
            .read_at(2, &mut buf)
            .expect("read with normalized segment size");

        assert_eq!(n, 3);
        assert_eq!(&buf, &[2, 3, 4]);
    }

    #[test]
    fn cached_ewf_source_preserves_full_stream_bytes() {
        let backing = Arc::new(TestEvidenceSource::new((0..128).map(|n| n as u8).collect()));
        let cached = CachedEwfSource::with_shards(Box::new(Arc::clone(&backing)), 16, 8, 4);

        let uncached_hash = compute_sha256(&backing, 11).expect("uncached hash");
        let cached_hash = compute_sha256(&cached, 11).expect("cached hash");

        assert_eq!(
            cached_hash, uncached_hash,
            "cached reads must be byte-identical"
        );
    }

    #[test]
    fn cached_ewf_source_supports_concurrent_reads() {
        let data: Vec<u8> = (0..128).map(|n| n as u8).collect();
        let backing = Arc::new(TestEvidenceSource::new(data.clone()));
        let cached = Arc::new(CachedEwfSource::with_shards(
            Box::new(Arc::clone(&backing)),
            16,
            8,
            4,
        ));
        let barrier = Arc::new(Barrier::new(8));
        // These offsets deliberately map to distinct 8-byte segments, so the
        // exact backing-read count remains deterministic under concurrency.
        let offsets: Vec<usize> = (0..8).map(|idx| idx * 8).collect();
        let mut handles = Vec::new();

        for offset in offsets {
            let cached = Arc::clone(&cached);
            let barrier = Arc::clone(&barrier);
            let expected = data[offset..offset + 8].to_vec();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..50 {
                    let mut buf = [0u8; 8];
                    let n = cached
                        .read_at(offset as u64, &mut buf)
                        .expect("concurrent cache read");
                    assert_eq!(n, expected.len());
                    assert_eq!(&buf[..n], expected.as_slice());
                }
            }));
        }

        for handle in handles {
            handle.join().expect("thread panicked");
        }

        assert_eq!(
            backing.read_calls(),
            8,
            "expected one backing read per distinct segment"
        );
    }

    #[test]
    #[ignore = "manual benchmark harness for comparing cache shard counts"]
    fn cached_ewf_source_manual_sharding_benchmark() {
        use std::time::Instant;

        fn bench_cache(source: Arc<CachedEwfSource>, offsets: &[u64]) -> std::time::Duration {
            let barrier = Arc::new(Barrier::new(offsets.len()));
            let mut handles = Vec::with_capacity(offsets.len());

            for &offset in offsets {
                let source = Arc::clone(&source);
                let barrier = Arc::clone(&barrier);
                handles.push(std::thread::spawn(move || {
                    let mut buf = [0u8; 64];
                    barrier.wait();
                    let start = Instant::now();
                    for _ in 0..10_000 {
                        let n = source.read_at(offset, &mut buf).expect("benchmark read");
                        assert_eq!(n, buf.len());
                    }
                    start.elapsed()
                }));
            }

            handles
                .into_iter()
                .map(|handle| handle.join().expect("benchmark thread panicked"))
                .max()
                .expect("at least one benchmark thread")
        }

        let data: Vec<u8> = (0..(64 * 32)).map(|n| (n % 251) as u8).collect();
        let offsets: Vec<u64> = (0..8).map(|idx| (idx * 64) as u64).collect();

        let single_shard = Arc::new(CachedEwfSource::with_shards(
            Box::new(Arc::new(TestEvidenceSource::new(data.clone()))),
            64,
            64,
            1,
        ));
        let sharded = Arc::new(CachedEwfSource::with_shards(
            Box::new(Arc::new(TestEvidenceSource::new(data))),
            64,
            64,
            8,
        ));

        let single = bench_cache(single_shard, &offsets);
        let multi = bench_cache(sharded, &offsets);

        eprintln!("single_shard={single:?} sharded={multi:?}");
    }

    #[cfg(not(feature = "ewf"))]
    #[test]
    fn ewf_requires_feature() {
        use std::fs;

        use crate::cli::{CliOptions, MetadataBackend};
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("image.E01");
        fs::write(&path, b"not ewf").expect("write");

        let opts = CliOptions {
            input: path,
            output: tmp.path().to_path_buf(),
            config_path: None,
            gpu: false,
            workers: 1,
            chunk_size_mib: 1,
            overlap_kib: None,
            metadata_backend: MetadataBackend::Jsonl,
            log_format: crate::cli::LogFormat::Text,
            progress_interval_secs: 0,
            scan_strings: false,
            scan_utf16: false,
            scan_urls: false,
            no_scan_urls: false,
            scan_emails: false,
            no_scan_emails: false,
            scan_phones: false,
            no_scan_phones: false,
            string_min_len: None,
            scan_entropy: false,
            entropy_window_bytes: None,
            entropy_threshold: None,
            max_bytes: None,
            max_chunks: None,
            max_files: None,
            max_memory_mib: None,
            max_open_files: None,
            checkpoint_path: None,
            resume_from: None,
            evidence_sha256: None,
            compute_evidence_sha256: false,
            disable_zip: false,
            types: None,
            enable_types: None,
            dry_run: false,
            validate_carved: false,
            remove_invalid: false,
            hash_algorithms: None,
            dedupe: false,
            skip_duplicates: false,
            metadata_only: false,
            write_workers: None,
            scan_workers: None,
            carve_workers: None,
        };

        let result = super::open_source(&opts, 1);
        match result {
            Ok(_) => panic!("expected unsupported error"),
            Err(super::EvidenceError::Unsupported(_)) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
}
