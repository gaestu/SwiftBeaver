use std::fs::{File, OpenOptions};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvidenceError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
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
                .map_err(|_| EvidenceError::Unsupported("raw file lock poisoned".to_string()))?;
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
            return Ok(Self {
                file,
                len,
                #[cfg(not(unix))]
                lock: std::sync::Mutex::new(()),
            });
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
                .map_err(|_| EvidenceError::Unsupported("device lock poisoned".to_string()))?;
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

            let guard = self.handle.lock().map_err(|_| {
                EvidenceError::Unsupported("libewf handle lock poisoned".to_string())
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

pub fn open_source(opts: &CliOptions) -> Result<Box<dyn EvidenceSource>, EvidenceError> {
    if is_ewf_path(&opts.input) {
        #[cfg(feature = "ewf")]
        {
            let src = ewf::EwfSource::open(&opts.input)?;
            return Ok(Box::new(src));
        }
        #[cfg(not(feature = "ewf"))]
        {
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

fn is_ewf_path(path: &std::path::Path) -> bool {
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

#[cfg(test)]
mod tests {
    use super::{RawFileSource, compute_sha256, is_ewf_path};

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
            scan_sqlite_pages: false,
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
        };

        let result = super::open_source(&opts);
        match result {
            Ok(_) => panic!("expected unsupported error"),
            Err(super::EvidenceError::Unsupported(_)) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
}
