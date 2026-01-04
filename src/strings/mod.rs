pub mod cpu;
#[cfg(feature = "gpu-cuda")]
pub mod cuda;
#[cfg(feature = "gpu-opencl")]
pub mod opencl;

use crate::chunk::ScanChunk;

#[derive(Debug, Clone)]
pub struct StringSpan {
    pub chunk_id: u64,
    pub local_start: u64,
    pub length: u32,
    pub flags: u32,
}

pub mod flags {
    pub const UTF16_LE: u32 = 1 << 0;
    pub const UTF16_BE: u32 = 1 << 1;
    pub const UTF8: u32 = 1 << 2;
    pub const URL_LIKE: u32 = 1 << 4;
    pub const EMAIL_LIKE: u32 = 1 << 5;
    pub const PHONE_LIKE: u32 = 1 << 6;
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
        cfg.string_scan_utf16,
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
    use crate::strings::flags;
    use once_cell::sync::Lazy;
    use regex::Regex;
    use serde::Serialize;

    #[derive(Debug, Clone, Copy)]
    pub struct ArtefactScanConfig {
        pub urls: bool,
        pub emails: bool,
        pub phones: bool,
    }

    impl ArtefactScanConfig {
        pub fn all() -> Self {
            Self {
                urls: true,
                emails: true,
                phones: true,
            }
        }
    }

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

    static URL_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"(?i)\b(?:https?://|www\.)[^\s"'<>]+"#).expect("url regex"));
    static EMAIL_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b").expect("email regex")
    });
    static PHONE_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\b\+?\d[\d\s().-]{6,}\d\b").expect("phone regex"));

    pub fn extract_artefacts(
        run_id: &str,
        chunk_start: u64,
        local_start: u64,
        flags: u32,
        data: &[u8],
        scan_cfg: ArtefactScanConfig,
    ) -> Vec<StringArtefact> {
        let mut out = Vec::new();
        let (text, encoding) = decode_span(flags, data);
        let hint_mask = flags::URL_LIKE | flags::EMAIL_LIKE | flags::PHONE_LIKE;
        let use_hints = (flags & hint_mask) != 0;

        if scan_cfg.urls && (!use_hints || (flags & flags::URL_LIKE) != 0) {
            for mat in URL_RE.find_iter(&text) {
                if let Some(value) = normalize_url(mat.as_str()) {
                    out.push(build_artefact(
                        run_id,
                        ArtefactKind::Url,
                        &value,
                        &encoding,
                        chunk_start + local_start + mat.start() as u64,
                    ));
                }
            }
        }

        if scan_cfg.emails && (!use_hints || (flags & flags::EMAIL_LIKE) != 0) {
            for mat in EMAIL_RE.find_iter(&text) {
                if let Some(value) = normalize_email(mat.as_str()) {
                    out.push(build_artefact(
                        run_id,
                        ArtefactKind::Email,
                        &value,
                        &encoding,
                        chunk_start + local_start + mat.start() as u64,
                    ));
                }
            }
        }

        if scan_cfg.phones && (!use_hints || (flags & flags::PHONE_LIKE) != 0) {
            for mat in PHONE_RE.find_iter(&text) {
                let value = mat.as_str();
                if is_plausible_phone(value) {
                    out.push(build_artefact(
                        run_id,
                        ArtefactKind::Phone,
                        value,
                        &encoding,
                        chunk_start + local_start + mat.start() as u64,
                    ));
                }
            }
        }

        out
    }

    pub(crate) fn extract_urls_from_text(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for mat in URL_RE.find_iter(text) {
            if let Some(value) = normalize_url(mat.as_str()) {
                out.push(value);
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
        // Require at least 4 unique digits to filter low-entropy false positives
        // (e.g., "7676766773" has only 3 unique digits: 7, 6, 3)
        let unique: std::collections::HashSet<_> = digits.iter().collect();
        if unique.len() < 4 {
            return false;
        }
        true
    }

    fn build_artefact(
        run_id: &str,
        kind: ArtefactKind,
        content: &str,
        encoding: &str,
        global_start: u64,
    ) -> StringArtefact {
        let len = content.as_bytes().len() as u64;
        let global_end = if len == 0 {
            global_start
        } else {
            global_start + len - 1
        };
        StringArtefact {
            run_id: run_id.to_string(),
            artefact_kind: kind,
            content: content.to_string(),
            encoding: encoding.to_string(),
            global_start,
            global_end,
        }
    }

    fn decode_span(flags: u32, data: &[u8]) -> (std::borrow::Cow<'_, str>, &'static str) {
        if (flags & flags::UTF16_LE) != 0 {
            let decoded = decode_utf16_bytes(data, true);
            return (std::borrow::Cow::Owned(decoded), "utf-16le");
        }
        if (flags & flags::UTF16_BE) != 0 {
            let decoded = decode_utf16_bytes(data, false);
            return (std::borrow::Cow::Owned(decoded), "utf-16be");
        }
        if (flags & flags::UTF8) != 0 {
            return (String::from_utf8_lossy(data), "utf-8");
        }
        (String::from_utf8_lossy(data), "ascii")
    }

    fn decode_utf16_bytes(data: &[u8], little_endian: bool) -> String {
        let mut out = Vec::with_capacity(data.len() / 2);
        let start = if little_endian { 0 } else { 1 };
        let mut i = start;
        while i < data.len() {
            out.push(data[i]);
            i += 2;
        }
        String::from_utf8_lossy(&out).to_string()
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
        use super::{ArtefactKind, ArtefactScanConfig, extract_artefacts};
        use crate::strings::flags;

        #[test]
        fn extracts_basic_artefacts() {
            let data = b"visit https://example.com and mail test@example.com";
            let out = extract_artefacts("run1", 100, 0, 0, data, ArtefactScanConfig::all());
            assert!(
                out.iter()
                    .any(|a| matches!(a.artefact_kind, ArtefactKind::Url))
            );
            assert!(
                out.iter()
                    .any(|a| matches!(a.artefact_kind, ArtefactKind::Email))
            );
        }

        #[test]
        fn extracts_utf16le_url() {
            let text = "https://example.com";
            let mut data = Vec::new();
            for b in text.as_bytes() {
                data.push(*b);
                data.push(0);
            }
            let out = extract_artefacts(
                "run1",
                0,
                0,
                flags::UTF16_LE | flags::URL_LIKE,
                &data,
                ArtefactScanConfig::all(),
            );
            assert!(out.iter().any(|a| {
                matches!(a.artefact_kind, ArtefactKind::Url) && a.encoding == "utf-16le"
            }));
        }

        #[test]
        fn filters_noisy_phone_matches() {
            let data = b"0000000000 bad +1 (415) 555-1234 good";
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
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
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
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
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
            let emails: Vec<&str> = out
                .iter()
                .filter(|a| matches!(a.artefact_kind, ArtefactKind::Email))
                .map(|a| a.content.as_str())
                .collect();
            assert!(emails.contains(&"user@example.com"));
        }

        #[test]
        fn reports_utf8_encoding() {
            let data = b"https://example.com";
            let out = extract_artefacts(
                "run1",
                0,
                0,
                flags::UTF8 | flags::URL_LIKE,
                data,
                ArtefactScanConfig::all(),
            );
            assert!(out.iter().any(|a| a.encoding == "utf-8"));
        }

        #[test]
        fn respects_scan_config() {
            let data = b"https://example.com test@example.com";
            let out = extract_artefacts(
                "run1",
                0,
                0,
                0,
                data,
                ArtefactScanConfig {
                    urls: false,
                    emails: true,
                    phones: false,
                },
            );
            assert!(
                out.iter()
                    .all(|a| matches!(a.artefact_kind, ArtefactKind::Email))
            );
        }
    }
}
