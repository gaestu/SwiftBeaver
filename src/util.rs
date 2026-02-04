//! # Utility Module
//!
//! Utility functions for the SwiftBeaver crate, including file type filtering
//! and carve registry building.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::Path;

use anyhow::{Result, anyhow};
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

    let probe_path = path.join(".swiftbeaver_write_probe");
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
/// If dry_run is true, creates a registry that won't write files to disk
pub fn build_carve_registry(cfg: &Config, dry_run: bool) -> Result<CarveRegistry> {
    // For dry-run mode, we still need the registry to track hits but won't write files
    // The actual file writing is skipped in the carve handlers when dry_run is enabled
    let _ = dry_run; // Currently handled by not creating output dirs

    let mut handlers: HashMap<String, Box<dyn carve::CarveHandler>> = HashMap::new();
    let allow_quicktime = matches!(cfg.quicktime_mode, crate::config::QuicktimeMode::Mp4);
    let mut mp4_ext = "mp4".to_string();
    let mut has_mp4 = false;
    for file_type in &cfg.file_types {
        let validator = if file_type.validator.trim().is_empty() {
            file_type.id.as_str()
        } else {
            file_type.validator.as_str()
        };
        if validator == "mp4" {
            has_mp4 = true;
            if let Some(ext) = file_type.extensions.first() {
                mp4_ext = carve::sanitize_extension(ext);
            }
        }
    }

    fn boxed<T: carve::CarveHandler + 'static>(handler: T) -> Box<dyn carve::CarveHandler> {
        Box::new(handler)
    }

    type SimpleBuilder = fn(String, u64, u64) -> Box<dyn carve::CarveHandler>;
    let mut simple_builders: HashMap<&'static str, SimpleBuilder> = HashMap::new();
    simple_builders.insert("jpeg", |ext, min, max| {
        boxed(carve::jpeg::JpegCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("png", |ext, min, max| {
        boxed(carve::png::PngCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("gif", |ext, min, max| {
        boxed(carve::gif::GifCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("sqlite", |ext, min, max| {
        boxed(carve::sqlite::SqliteCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("pdf", |ext, min, max| {
        boxed(carve::pdf::PdfCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("gzip", |ext, min, max| {
        boxed(carve::gzip::GzipCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("bzip2", |ext, min, max| {
        boxed(carve::bzip2::Bzip2CarveHandler::new(ext, min, max))
    });
    simple_builders.insert("xz", |ext, min, max| {
        boxed(carve::xz::XzCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("tar", |ext, min, max| {
        boxed(carve::tar::TarCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("webp", |ext, min, max| {
        boxed(carve::webp::WebpCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("bmp", |ext, min, max| {
        boxed(carve::bmp::BmpCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("tiff", |ext, min, max| {
        boxed(carve::tiff::TiffCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("rar", |ext, min, max| {
        boxed(carve::rar::RarCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("sevenz", |ext, min, max| {
        boxed(carve::sevenz::SevenZCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("wav", |ext, min, max| {
        boxed(carve::wav::WavCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("avi", |ext, min, max| {
        boxed(carve::avi::AviCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("webm", |ext, min, max| {
        boxed(carve::webm::WebmCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("wmv", |ext, min, max| {
        boxed(carve::wmv::WmvCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("mp3", |ext, min, max| {
        boxed(carve::mp3::Mp3CarveHandler::new(ext, min, max))
    });
    simple_builders.insert("ogg", |ext, min, max| {
        boxed(carve::ogg::OggCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("rtf", |ext, min, max| {
        boxed(carve::rtf::RtfCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("ico", |ext, min, max| {
        boxed(carve::ico::IcoCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("elf", |ext, min, max| {
        boxed(carve::elf::ElfCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("eml", |ext, min, max| {
        boxed(carve::eml::EmlCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("mobi", |ext, min, max| {
        boxed(carve::mobi::MobiCarveHandler::new(ext, min, max))
    });
    simple_builders.insert("fb2", |ext, min, max| {
        boxed(carve::fb2::Fb2CarveHandler::new(ext, min, max))
    });
    simple_builders.insert("lrf", |ext, min, max| {
        boxed(carve::lrf::LrfCarveHandler::new(ext, min, max))
    });

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

        if let Some(builder) = simple_builders.get(validator) {
            handlers.insert(
                file_type.id.clone(),
                builder(ext, file_type.min_size, file_type.max_size),
            );
            continue;
        }

        match validator {
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
            "mp4" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::mp4::Mp4CarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                        allow_quicktime,
                    )),
                );
            }
            "mov" => {
                if allow_quicktime && has_mp4 {
                    debug!("mov handler skipped because quicktime_mode=mp4");
                    continue;
                }
                let handler: Box<dyn carve::CarveHandler> = if allow_quicktime {
                    Box::new(carve::mp4::Mp4CarveHandler::new(
                        mp4_ext.clone(),
                        file_type.min_size,
                        file_type.max_size,
                        true,
                    ))
                } else {
                    Box::new(carve::mov::MovCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                    ))
                };
                handlers.insert(file_type.id.clone(), handler);
            }
            "ole" => {
                handlers.insert(
                    file_type.id.clone(),
                    Box::new(carve::ole::OleCarveHandler::new(
                        ext,
                        file_type.min_size,
                        file_type.max_size,
                        cfg.ole_allowed_kinds.clone(),
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
        let mut has_ole = false;
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
            if file_type.id.eq_ignore_ascii_case("ole")
                || file_type.validator.eq_ignore_ascii_case("ole")
            {
                has_ole = true;
            }
        }
        if has_zip {
            for kind in ["zip", "docx", "xlsx", "pptx", "odt", "ods", "odp", "epub"] {
                known.insert(kind.to_string());
            }
        }
        if has_ole {
            for kind in ["ole", "doc", "xls", "ppt"] {
                known.insert(kind.to_string());
            }
        }

        for entry in &allow {
            if !known.contains(entry) {
                unknown.push(entry.clone());
            }
        }

        let allow_zip_family = allow.iter().any(|entry| is_zip_kind(entry));
        let allow_ole_family = allow.iter().any(|entry| is_ole_kind(entry));

        cfg.file_types.retain(|file_type| {
            let id = file_type.id.to_ascii_lowercase();
            let validator = if file_type.validator.trim().is_empty() {
                id.clone()
            } else {
                file_type.validator.to_ascii_lowercase()
            };
            let is_zip = id == "zip" || validator == "zip";
            let is_ole = id == "ole" || validator == "ole";
            allow.contains(&id)
                || allow.contains(&validator)
                || (is_zip && allow_zip_family)
                || (is_ole && allow_ole_family)
        });

        if allow_zip_family && has_zip {
            if allow.contains("zip") {
                cfg.zip_allowed_kinds = None;
            } else {
                let mut kinds = Vec::new();
                for kind in ["docx", "xlsx", "pptx", "odt", "ods", "odp", "epub"] {
                    if allow.contains(kind) {
                        kinds.push(kind.to_string());
                    }
                }
                cfg.zip_allowed_kinds = if kinds.is_empty() { None } else { Some(kinds) };
            }
        }
        if allow_ole_family && has_ole {
            if allow.contains("ole") {
                cfg.ole_allowed_kinds = None;
            } else {
                let mut kinds = Vec::new();
                for kind in ["doc", "xls", "ppt"] {
                    if allow.contains(kind) {
                        kinds.push(kind.to_string());
                    }
                }
                cfg.ole_allowed_kinds = if kinds.is_empty() { None } else { Some(kinds) };
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
    matches!(
        value,
        "zip" | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub"
    )
}

fn is_ole_kind(value: &str) -> bool {
    matches!(value, "ole" | "doc" | "xls" | "ppt")
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
        assert!(
            cfg.file_types
                .iter()
                .all(|ft| !ft.id.eq_ignore_ascii_case("zip"))
        );
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
