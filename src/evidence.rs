use std::fs::File;

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
            let _guard = self.lock.lock().unwrap();
            let mut f = &self.file;
            f.seek(SeekFrom::Start(offset))?;
            Ok(f.read(buf)?)
        }
    }
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
    extern "C" {
        fn libewf_get_access_flags_read() -> c_int;

        fn libewf_check_file_signature(filename: *const c_char, error: *mut *mut LibEwfError) -> c_int;
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

        fn libewf_handle_initialize(handle: *mut *mut LibEwfHandle, error: *mut *mut LibEwfError)
            -> c_int;
        fn libewf_handle_free(handle: *mut *mut LibEwfHandle, error: *mut *mut LibEwfError) -> c_int;
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

        fn libewf_error_sprint(error: *mut LibEwfError, string: *mut c_char, size: size_t) -> c_int;
        fn libewf_error_free(error: *mut *mut LibEwfError);
    }

    struct HandleInner {
        handle: *mut LibEwfHandle,
    }

    pub struct EwfSource {
        handle: Mutex<HandleInner>,
        len: u64,
    }

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
                let rc = libewf_handle_open(handle, filenames, number_of_filenames, access_flags, &mut error);
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

            let mut guard = self.handle.lock().unwrap();
            if guard.handle.is_null() {
                return Err(EvidenceError::Unsupported("libewf handle closed".to_string()));
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
        let rc = libewf_error_sprint(error, buf.as_mut_ptr(), buf.len());
        let msg = if rc >= 0 {
            CStr::from_ptr(buf.as_ptr())
                .to_string_lossy()
                .trim_end_matches('\0')
                .to_string()
        } else {
            "libewf error".to_string()
        };
        libewf_error_free(&mut error);
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

    let src = RawFileSource::open(&opts.input)?;
    Ok(Box::new(src))
}

fn is_ewf_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("e01"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::is_ewf_path;

    #[test]
    fn ewf_extension_detection() {
        assert!(is_ewf_path(std::path::Path::new("case.E01")));
        assert!(is_ewf_path(std::path::Path::new("case.e01")));
        assert!(!is_ewf_path(std::path::Path::new("case.dd")));
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
        };

        let result = super::open_source(&opts);
        match result {
            Ok(_) => panic!("expected unsupported error"),
            Err(super::EvidenceError::Unsupported(_)) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
}
