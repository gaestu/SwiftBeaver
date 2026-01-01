//! # Utility Module
//!
//! Utility functions for the fastcarve crate, including file type filtering
//! and carve registry building.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::Path;

use anyhow::{anyhow, Result};
#[cfg(unix)]
use tracing::info;
use tracing::{debug, warn};

use crate::carve::{self, CarveRegistry};
use crate::config::Config;
use crate::metadata::MetadataBackendKind;

/// Convert CLI metadata backend to internal enum
pub fn backend_from_cli(backend: crate::cli::MetadataBackend) -> MetadataBackendKind {
    match backend {
        crate::cli::MetadataBackend::Jsonl => MetadataBackendKind::Jsonl,
        crate::cli::MetadataBackend::Csv => MetadataBackendKind::Csv,
        crate::cli::MetadataBackend::Parquet => MetadataBackendKind::Parquet,
    }
}

/// Ensure output directory exists and is writable, warning on unsafe permissions.
pub fn ensure_output_dir(path: &Path) -> Result<()> {
    if path.exists() {
        let metadata = std::fs::metadata(path)?;
        if !metadata.is_dir() {
            return Err(anyhow!(
                "output path is not a directory: {}",
                path.display()
            ));
        }
    } else {
        std::fs::create_dir_all(path)?;
    }
    let metadata = std::fs::metadata(path)?;

    let probe_path = path.join(".fastcarve_write_probe");
    match OpenOptions::new()
        .write(true)
        .create(true)
        .open(&probe_path)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe_path);
        }
        Err(err) => {
            return Err(anyhow!(
                "output directory is not writable: {} ({})",
                path.display(),
                err
            ));
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        if mode & 0o002 != 0 {
            warn!("output directory is world-writable: {}", path.display());
        }
    }

    Ok(())
}

/// Apply optional resource limits for this process.
pub fn apply_resource_limits(
    max_memory_mib: Option<u64>,
    max_open_files: Option<u64>,
) -> Result<()> {
    #[cfg(unix)]
    {
        if let Some(mem_mib) = max_memory_mib {
            let bytes = mem_mib.saturating_mul(1024 * 1024);
            set_limit(libc::RLIMIT_AS, bytes, "address space")?;
        }
        if let Some(open_files) = max_open_files {
            set_limit(libc::RLIMIT_NOFILE, open_files, "open file descriptors")?;
        }
    }
    #[cfg(not(unix))]
    {
        if max_memory_mib.is_some() || max_open_files.is_some() {
            warn!("resource limits are only supported on Unix platforms");
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_limit(resource: libc::__rlimit_resource_t, requested: u64, label: &str) -> Result<()> {
    unsafe {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(resource, &mut limit) != 0 {
            return Err(anyhow!(
                "getrlimit failed for {}: {}",
                label,
                std::io::Error::last_os_error()
            ));
        }

        let requested = requested as libc::rlim_t;
        let mut new_cur = requested;
        if requested > limit.rlim_max {
            warn!(
                "requested {} limit {} exceeds hard limit {}; using {}",
                label, requested, limit.rlim_max, limit.rlim_max
            );
            new_cur = limit.rlim_max;
        }

        let new_limit = libc::rlimit {
            rlim_cur: new_cur,
            rlim_max: limit.rlim_max,
        };

        if libc::setrlimit(resource, &new_limit) != 0 {
            return Err(anyhow!(
                "setrlimit failed for {}: {}",
                label,
                std::io::Error::last_os_error()
            ));
        }
        info!("set {} limit to {}", label, new_cur);
    }
    Ok(())
}

/// Build the carve registry from configuration
pub fn build_carve_registry(cfg: &Config) -> Result<CarveRegistry> {
    let mut handlers: HashMap<String, Box<dyn carve::CarveHandler>> = HashMap::new();

    for file_type in &cfg.file_types {
        let validator = if file_type.validator.trim().is_empty() {
            file_type.id.as_str()
        } else {
            file_type.validator.as_str()
        };
        let ext = file_type
            .extensions
            .first()
            .cloned()
            .unwrap_or_else(|| file_type.id.clone());
        let ext = carve::sanitize_extension(&ext);

        if !file_type.footer_patterns.is_empty() && validator != "footer" {
            debug!(
                "footer patterns configured for file_type={} but validator={} does not use them",
                file_type.id, validator
            );
        }

        match validator {
            "jpeg" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::jpeg::JpegCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "png" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::png::PngCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "gif" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::gif::GifCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "sqlite" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::sqlite::SqliteCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "pdf" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::pdf::PdfCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "zip" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::zip::ZipCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                        file_type.require_eocd,
                        cfg.zip_allowed_kinds.clone(),
                    )),
                );
            }
            "webp" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::webp::WebpCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "bmp" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::bmp::BmpCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "tiff" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::tiff::TiffCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "mp4" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::mp4::Mp4CarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "rar" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::rar::RarCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "sevenz" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::sevenz::SevenZCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    )),
                );
            }
            "footer" => {
                let headers = decode_patterns(&file_type.header_patterns, &file_type.id, "header")?;
                let footers = decode_patterns(&file_type.footer_patterns, &file_type.id, "footer")?;
                if headers.is_empty() {
                    debug!(
                        "footer handler skipped for file_type={} (no header patterns)",
                        file_type.id
                    );
                    continue;
                }
                if footers.is_empty() {
                    debug!(
                        "footer handler skipped for file_type={} (no footer patterns)",
                        file_type.id
                    );
                    continue;
                }
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::footer::FooterCarveHandler::new(
                        file_type.id.clone(),
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                        headers,
                        footers,
                    )),
                );
            }
            _ => {
                debug!(
                    "no carve handler for file_type={} validator={}",
                    file_type.id, validator
                );
            }
        }
    }

    Ok(CarveRegistry::new(handlers))
}

fn decode_patterns(
    patterns: &[crate::config::PatternConfig],
    file_type: &str,
    kind: &str,
) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    for pattern in patterns {
        let bytes = hex::decode(pattern.hex.trim()).map_err(|e| {
            anyhow!(
                "invalid {} pattern {} for file_type {}: {e}",
                kind,
                pattern.id,
                file_type
            )
        })?;
        if !bytes.is_empty() {
            out.push(bytes);
        }
    }
    Ok(out)
}

/// Filter file types based on allow list and disable flags
pub fn filter_file_types(
    cfg: &mut Config,
    allow_list: Option<&[String]>,
    disable_zip: bool,
) -> Vec<String> {
    use std::collections::HashSet;

    let mut unknown = Vec::new();
    if let Some(list) = allow_list {
        let mut allow = HashSet::new();
        for entry in list {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                continue;
            }
            allow.insert(trimmed.to_ascii_lowercase());
        }

        let mut known = HashSet::new();
        let mut has_zip = false;
        for file_type in &cfg.file_types {
            known.insert(file_type.id.to_ascii_lowercase());
            if !file_type.validator.trim().is_empty() {
                known.insert(file_type.validator.to_ascii_lowercase());
            }
            if file_type.id.eq_ignore_ascii_case("zip")
                || file_type.validator.eq_ignore_ascii_case("zip")
            {
                has_zip = true;
            }
        }
        if has_zip {
            for kind in ["zip", "docx", "xlsx", "pptx"] {
                known.insert(kind.to_string());
            }
        }

        for entry in &allow {
            if !known.contains(entry) {
                unknown.push(entry.clone());
            }
        }

        let allow_zip_family = allow.iter().any(|entry| is_zip_kind(entry));

        cfg.file_types.retain(|file_type| {
            let id = file_type.id.to_ascii_lowercase();
            let validator = if file_type.validator.trim().is_empty() {
                id.clone()
            } else {
                file_type.validator.to_ascii_lowercase()
            };
            let is_zip = id == "zip" || validator == "zip";
            allow.contains(&id) || allow.contains(&validator) || (is_zip && allow_zip_family)
        });

        if allow_zip_family && has_zip {
            if allow.contains("zip") {
                cfg.zip_allowed_kinds = None;
            } else {
                let mut kinds = Vec::new();
                for kind in ["docx", "xlsx", "pptx"] {
                    if allow.contains(kind) {
                        kinds.push(kind.to_string());
                    }
                }
                cfg.zip_allowed_kinds = if kinds.is_empty() { None } else { Some(kinds) };
            }
        }
    }

    if disable_zip {
        cfg.file_types.retain(|file_type| {
            let is_zip = file_type.id.eq_ignore_ascii_case("zip")
                || file_type.validator.eq_ignore_ascii_case("zip");
            !is_zip
        });
        cfg.zip_allowed_kinds = None;
    }

    unknown.sort();
    unknown
}

fn is_zip_kind(value: &str) -> bool {
    matches!(value, "zip" | "docx" | "xlsx" | "pptx")
}

#[cfg(test)]
mod tests {
    use super::{ensure_output_dir, filter_file_types};
    use crate::config;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn filters_allowed_types() {
        let loaded = config::load_config(None).expect("config");
        let mut cfg = loaded.config;
        let unknown = filter_file_types(
            &mut cfg,
            Some(&["jpeg".to_string(), "sqlite".to_string()]),
            false,
        );
        assert!(unknown.is_empty());
        let ids: Vec<&str> = cfg.file_types.iter().map(|ft| ft.id.as_str()).collect();
        assert_eq!(ids, vec!["jpeg", "sqlite"]);
    }

    #[test]
    fn disable_zip_removes_zip_validator() {
        let loaded = config::load_config(None).expect("config");
        let mut cfg = loaded.config;
        let _ = filter_file_types(&mut cfg, Some(&["zip".to_string()]), true);
        assert!(cfg
            .file_types
            .iter()
            .all(|ft| !ft.id.eq_ignore_ascii_case("zip")));
    }

    #[test]
    fn reports_unknown_types() {
        let loaded = config::load_config(None).expect("config");
        let mut cfg = loaded.config;
        let unknown = filter_file_types(
            &mut cfg,
            Some(&["jpeg".to_string(), "nope".to_string()]),
            false,
        );
        assert_eq!(unknown, vec!["nope"]);
    }

    #[test]
    fn allows_docx_through_zip_handler() {
        let loaded = config::load_config(None).expect("config");
        let mut cfg = loaded.config;
        let unknown = filter_file_types(&mut cfg, Some(&["docx".to_string()]), false);
        assert!(unknown.is_empty());
        let ids: Vec<&str> = cfg.file_types.iter().map(|ft| ft.id.as_str()).collect();
        assert_eq!(ids, vec!["zip"]);
        let mut kinds = cfg.zip_allowed_kinds.unwrap_or_default();
        kinds.sort();
        assert_eq!(kinds, vec!["docx"]);
    }

    #[test]
    fn ensures_output_dir_is_writable() {
        let dir = tempdir().expect("tempdir");
        ensure_output_dir(dir.path()).expect("ensure output dir");
    }

    #[test]
    fn rejects_output_path_that_is_file() {
        let dir = tempdir().expect("tempdir");
        let file_path = dir.path().join("output.txt");
        let _ = File::create(&file_path).expect("create file");
        let err = ensure_output_dir(&file_path).expect_err("should fail");
        assert!(err.to_string().contains("not a directory"));
    }
}
