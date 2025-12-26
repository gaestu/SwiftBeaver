pub mod cpu;
#[cfg(feature = "gpu-opencl")]
pub mod opencl;
#[cfg(feature = "gpu-cuda")]
pub mod cuda;

use crate::chunk::ScanChunk;

#[derive(Debug, Clone)]
pub struct StringSpan {
    pub chunk_id: u64,
    pub local_start: u64,
    pub length: u32,
    pub flags: u32,
}

pub trait StringScanner: Send + Sync {
    fn scan_chunk(&self, chunk: &ScanChunk, data: &[u8]) -> Vec<StringSpan>;
}

use crate::config::Config;
use anyhow::Result;
use tracing::warn;

pub fn build_string_scanner(cfg: &Config, use_gpu: bool) -> Result<Box<dyn StringScanner>> {
    if use_gpu {
        #[cfg(feature = "gpu-opencl")]
        {
            match opencl::OpenClStringScanner::new(cfg) {
                Ok(scanner) => return Ok(Box::new(scanner)),
                Err(err) => warn!("opencl string scanner init failed: {err}; falling back to cpu"),
            }
        }
        #[cfg(feature = "gpu-cuda")]
        {
            match cuda::CudaStringScanner::new(cfg) {
                Ok(scanner) => return Ok(Box::new(scanner)),
                Err(err) => warn!("cuda string scanner init failed: {err}; falling back to cpu"),
            }
        }
        #[cfg(not(any(feature = "gpu-opencl", feature = "gpu-cuda")))]
        {
            warn!("gpu flag set but binary built without gpu feature, falling back to cpu");
        }
    }

    Ok(Box::new(cpu::CpuStringScanner::new(
        cfg.string_min_len,
        cfg.string_max_len,
    )))
}

#[cfg(test)]
mod build_tests {
    use super::build_string_scanner;
    use crate::config;

    #[test]
    fn builds_string_scanner_with_gpu_flag() {
        let loaded = config::load_config(None).expect("config");
        let scanner = build_string_scanner(&loaded.config, true).expect("scanner");
        let _ = scanner;
    }
}

pub mod artifacts {
    use once_cell::sync::Lazy;
    use regex::Regex;
    use serde::Serialize;

    #[derive(Debug, Clone, Serialize)]
    pub enum ArtefactKind {
        Url,
        Email,
        Phone,
        GenericString,
    }

    #[derive(Debug, Clone, Serialize)]
    pub struct StringArtefact {
        pub run_id: String,
        pub artefact_kind: ArtefactKind,
        pub content: String,
        pub encoding: String,
        pub global_start: u64,
        pub global_end: u64,
    }

    static URL_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"(?i)\b(?:https?://|www\.)[^\s"'<>]+"#).expect("url regex")
    });
    static EMAIL_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b").expect("email regex")
    });
    static PHONE_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"\b\+?\d[\d\s().-]{6,}\d\b").expect("phone regex")
    });

    pub fn extract_artefacts(
        run_id: &str,
        chunk_start: u64,
        local_start: u64,
        data: &[u8],
    ) -> Vec<StringArtefact> {
        let mut out = Vec::new();
        let text = String::from_utf8_lossy(data);

        for mat in URL_RE.find_iter(&text) {
            if let Some(value) = normalize_url(mat.as_str()) {
                out.push(build_artefact(
                    run_id,
                    ArtefactKind::Url,
                    &value,
                    chunk_start + local_start + mat.start() as u64,
                ));
            }
        }

        for mat in EMAIL_RE.find_iter(&text) {
            if let Some(value) = normalize_email(mat.as_str()) {
                out.push(build_artefact(
                    run_id,
                    ArtefactKind::Email,
                    &value,
                    chunk_start + local_start + mat.start() as u64,
                ));
            }
        }

        for mat in PHONE_RE.find_iter(&text) {
            let value = mat.as_str();
            if is_plausible_phone(value) {
                out.push(build_artefact(
                    run_id,
                    ArtefactKind::Phone,
                    value,
                    chunk_start + local_start + mat.start() as u64,
                ));
            }
        }

        out
    }

    fn is_plausible_phone(value: &str) -> bool {
        let digits: Vec<char> = value.chars().filter(|c| c.is_ascii_digit()).collect();
        let len = digits.len();
        if len < 10 || len > 15 {
            return false;
        }
        if digits.is_empty() {
            return false;
        }
        let first = digits[0];
        if digits.iter().all(|d| *d == first) {
            return false;
        }
        true
    }

    fn build_artefact(run_id: &str, kind: ArtefactKind, content: &str, global_start: u64) -> StringArtefact {
        let len = content.as_bytes().len() as u64;
        let global_end = if len == 0 { global_start } else { global_start + len - 1 };
        StringArtefact {
            run_id: run_id.to_string(),
            artefact_kind: kind,
            content: content.to_string(),
            encoding: "ascii".to_string(),
            global_start,
            global_end,
        }
    }

    fn normalize_url(value: &str) -> Option<String> {
        let trimmed = trim_trailing_punct(value);
        if trimmed.len() < 8 || trimmed.len() > 2048 {
            return None;
        }
        let lower = trimmed.to_ascii_lowercase();
        let rest = if lower.starts_with("http://") {
            &trimmed[7..]
        } else if lower.starts_with("https://") {
            &trimmed[8..]
        } else if lower.starts_with("www.") {
            &trimmed[4..]
        } else {
            return None;
        };

        let host_end = rest.find('/').unwrap_or(rest.len());
        let host_port = &rest[..host_end];
        let host = host_port.split(':').next().unwrap_or("");
        if host.is_empty() || host.len() > 253 || !host.contains('.') {
            return None;
        }
        for part in host.split('.') {
            if part.is_empty() || part.len() > 63 {
                return None;
            }
        }

        Some(trimmed.to_string())
    }

    fn normalize_email(value: &str) -> Option<String> {
        let trimmed = trim_trailing_punct(value);
        if trimmed.len() < 6 || trimmed.len() > 254 {
            return None;
        }
        let (local, domain) = trimmed.split_once('@')?;
        if local.is_empty() || local.len() > 64 {
            return None;
        }
        if domain.len() > 253 || !domain.contains('.') {
            return None;
        }
        if !domain.chars().any(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        for part in domain.split('.') {
            if part.is_empty() || part.len() > 63 {
                return None;
            }
        }
        Some(trimmed.to_string())
    }

    fn trim_trailing_punct(value: &str) -> &str {
        value.trim_end_matches(|c: char| {
            matches!(
                c,
                '.' | ',' | ';' | ':' | ')' | ']' | '}' | '"' | '\'' | '>' | '<'
            )
        })
    }

    #[cfg(test)]
    mod tests {
        use super::{extract_artefacts, ArtefactKind};

        #[test]
        fn extracts_basic_artefacts() {
            let data = b"visit https://example.com and mail test@example.com";
            let out = extract_artefacts("run1", 100, 0, data);
            assert!(out.iter().any(|a| matches!(a.artefact_kind, ArtefactKind::Url)));
            assert!(out.iter().any(|a| matches!(a.artefact_kind, ArtefactKind::Email)));
        }

        #[test]
        fn filters_noisy_phone_matches() {
            let data = b"0000000000 bad +1 (415) 555-1234 good";
            let out = extract_artefacts("run1", 0, 0, data);
            let phones: Vec<&str> = out
                .iter()
                .filter(|a| matches!(a.artefact_kind, ArtefactKind::Phone))
                .map(|a| a.content.as_str())
                .collect();
            assert!(phones.iter().any(|v| v.contains("415")));
            assert!(!phones.iter().any(|v| v.starts_with("0000")));
        }

        #[test]
        fn trims_url_trailing_punct() {
            let data = b"(https://example.com/login),";
            let out = extract_artefacts("run1", 0, 0, data);
            let urls: Vec<&str> = out
                .iter()
                .filter(|a| matches!(a.artefact_kind, ArtefactKind::Url))
                .map(|a| a.content.as_str())
                .collect();
            assert!(urls.contains(&"https://example.com/login"));
        }

        #[test]
        fn trims_email_trailing_punct() {
            let data = b"user@example.com.";
            let out = extract_artefacts("run1", 0, 0, data);
            let emails: Vec<&str> = out
                .iter()
                .filter(|a| matches!(a.artefact_kind, ArtefactKind::Email))
                .map(|a| a.content.as_str())
                .collect();
            assert!(emails.contains(&"user@example.com"));
        }
    }
}
