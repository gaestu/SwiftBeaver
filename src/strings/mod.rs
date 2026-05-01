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
        pub bitlocker_recovery_passwords: bool,
    }

    impl ArtefactScanConfig {
        pub fn all() -> Self {
            Self {
                urls: true,
                emails: true,
                phones: true,
                bitlocker_recovery_passwords: true,
            }
        }
    }

    #[derive(Debug, Clone, Serialize)]
    pub enum ArtefactKind {
        Url,
        Email,
        Phone,
        BitlockerRecoveryPassword,
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
        Regex::new(r#"(?i)(?:https?://|ftps?://|www\.|//)[^\s"'<>]+"#).expect("url regex")
    });
    static EMAIL_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b").expect("email regex")
    });
    static PHONE_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\b\+?\d[\d\s().-]{6,}\d\b").expect("phone regex"));

    // Eight groups of six decimal digits separated by a single hyphen or ASCII
    // whitespace character (space, tab, CR, LF). Uniformity of separator and
    // arithmetic validation (group % 11 == 0, group / 11 <= u16::MAX) are
    // checked in `validate_bitlocker_recovery_password`.
    static BITLOCKER_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"\b\d{6}[- \t\r\n]\d{6}[- \t\r\n]\d{6}[- \t\r\n]\d{6}[- \t\r\n]\d{6}[- \t\r\n]\d{6}[- \t\r\n]\d{6}[- \t\r\n]\d{6}\b",
        )
        .expect("bitlocker recovery regex")
    });

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

        let should_scan_urls = scan_cfg.urls
            && (!use_hints
                || (flags & flags::URL_LIKE) != 0
                || text.contains("://")
                || text.contains("//")
                || text.to_ascii_lowercase().contains("www."));

        if should_scan_urls {
            for mat in URL_RE.find_iter(&text) {
                if let Some(value) = normalize_url(mat.as_str()) {
                    out.push(build_artefact(
                        run_id,
                        ArtefactKind::Url,
                        &value,
                        encoding,
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
                        encoding,
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
                        encoding,
                        chunk_start + local_start + mat.start() as u64,
                    ));
                }
            }
        }

        if scan_cfg.bitlocker_recovery_passwords {
            let text_bytes = text.as_bytes();
            for mat in BITLOCKER_RE.find_iter(&text) {
                let start = mat.start();
                let end = mat.end();
                // Reject candidates that are part of a longer digit-group run.
                // The regex `\b` boundary already excludes word characters
                // (letters, digits, underscore) immediately adjacent to the
                // match, but a trailing `<sep><digit>` sequence would still
                // satisfy `\b` and let an 8-group prefix of a 9-group run
                // match. Explicitly reject that case.
                if end + 1 < text_bytes.len()
                    && matches!(text_bytes[end], b'-' | b' ' | b'\t' | b'\r' | b'\n')
                    && text_bytes[end + 1].is_ascii_digit()
                {
                    continue;
                }
                if start >= 2
                    && matches!(text_bytes[start - 1], b'-' | b' ' | b'\t' | b'\r' | b'\n')
                    && text_bytes[start - 2].is_ascii_digit()
                {
                    continue;
                }
                if let Some(canonical) = validate_bitlocker_recovery_password(mat.as_str()) {
                    // Compute source-byte coordinates from the original span
                    // encoding. The regex matched offsets in the *decoded*
                    // text; for UTF-16 spans every decoded byte corresponds
                    // to two source bytes, so deriving end from the canonical
                    // ASCII length would record half-sized evidence
                    // coordinates.
                    let bytes_per_char: u64 = match encoding {
                        "utf-16le" | "utf-16be" => 2,
                        _ => 1,
                    };
                    let span_byte_len = (end - start) as u64 * bytes_per_char;
                    let global_start = chunk_start + local_start + (start as u64) * bytes_per_char;
                    let global_end = if span_byte_len == 0 {
                        global_start
                    } else {
                        global_start + span_byte_len - 1
                    };
                    out.push(StringArtefact {
                        run_id: run_id.to_string(),
                        artefact_kind: ArtefactKind::BitlockerRecoveryPassword,
                        content: canonical,
                        encoding: encoding.to_string(),
                        global_start,
                        global_end,
                    });
                }
            }
        }

        out
    }

    /// Validate a candidate BitLocker recovery password and return its canonical
    /// hyphen-separated form, or `None` if the candidate is not a valid recovery
    /// password.
    ///
    /// The candidate must consist of exactly eight groups of six decimal digits
    /// separated by a single uniform separator (all hyphens or all ASCII
    /// whitespace characters). Each group must be divisible by 11, and the
    /// quotient must fit in `u16` (i.e. each group must be `<= 720_885`).
    pub(crate) fn validate_bitlocker_recovery_password(candidate: &str) -> Option<String> {
        let bytes = candidate.as_bytes();
        // 8 groups of 6 digits + 7 single-byte separators = 55 bytes.
        if bytes.len() != 55 {
            return None;
        }

        let mut groups: [u32; 8] = [0; 8];
        let mut separator: Option<SeparatorClass> = None;
        for (i, group) in groups.iter_mut().enumerate() {
            let start = i * 7;
            let group_bytes = &bytes[start..start + 6];
            let mut value: u32 = 0;
            for &b in group_bytes {
                if !b.is_ascii_digit() {
                    return None;
                }
                value = value * 10 + u32::from(b - b'0');
            }
            *group = value;

            if i < 7 {
                let sep = bytes[start + 6];
                let class = SeparatorClass::classify(sep)?;
                match separator {
                    None => separator = Some(class),
                    Some(existing) if existing == class => {}
                    Some(_) => return None,
                }
            }
        }

        for &group in &groups {
            if group % 11 != 0 {
                return None;
            }
            let quotient = group / 11;
            if quotient > u32::from(u16::MAX) {
                return None;
            }
        }

        let mut canonical = String::with_capacity(55);
        for (i, group) in groups.iter().enumerate() {
            if i > 0 {
                canonical.push('-');
            }
            // Each group is 6 digits with leading zeros preserved.
            let s = format!("{:06}", group);
            canonical.push_str(&s);
        }
        Some(canonical)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SeparatorClass {
        Hyphen,
        Whitespace,
    }

    impl SeparatorClass {
        fn classify(byte: u8) -> Option<Self> {
            match byte {
                b'-' => Some(Self::Hyphen),
                b' ' | b'\t' | b'\r' | b'\n' => Some(Self::Whitespace),
                _ => None,
            }
        }
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
        if !(10..=15).contains(&len) {
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
        let len = content.len() as u64;
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
        let (rest, canonical) = if lower.starts_with("http://") {
            (&trimmed[7..], trimmed.to_string())
        } else if lower.starts_with("https://") {
            (&trimmed[8..], trimmed.to_string())
        } else if lower.starts_with("ftp://") {
            (&trimmed[6..], trimmed.to_string())
        } else if lower.starts_with("ftps://") {
            (&trimmed[7..], trimmed.to_string())
        } else if lower.starts_with("www.") {
            (&trimmed[4..], format!("http://{trimmed}"))
        } else if let Some(stripped) = trimmed.strip_prefix("//") {
            (stripped, format!("http:{trimmed}"))
        } else {
            return None;
        };

        let host_end = rest
            .find(|c| ['/', '?', '#'].contains(&c))
            .unwrap_or(rest.len());
        let authority = &rest[..host_end];
        if authority.is_empty() {
            return None;
        }

        let host_port = authority.rsplit('@').next().unwrap_or("");
        if host_port.is_empty() {
            return None;
        }

        let host = if host_port.starts_with('[') {
            let end = host_port.find(']')?;
            &host_port[1..end]
        } else {
            host_port.split(':').next().unwrap_or("")
        };

        if !is_valid_url_host(host) {
            return None;
        }

        Some(canonical)
    }

    fn is_valid_url_host(host: &str) -> bool {
        if host.is_empty() || host.len() > 253 {
            return false;
        }
        if host.parse::<std::net::IpAddr>().is_ok() {
            return true;
        }
        let labels: Vec<&str> = host.split('.').collect();
        if labels.is_empty() {
            return false;
        }
        for part in &labels {
            if !is_valid_domain_label(part) {
                return false;
            }
        }
        if labels.len() == 1 {
            let label = labels[0];
            if !label.chars().any(|c| c.is_ascii_alphabetic()) {
                return false;
            }
            if label.len() < 2 {
                return false;
            }
        }
        true
    }

    fn is_valid_domain_label(label: &str) -> bool {
        if label.is_empty() || label.len() > 63 {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
        label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
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
        fn extracts_ftp_url() {
            let data = b"ftp://downloads.example.com/tool.zip";
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
            let urls: Vec<&str> = out
                .iter()
                .filter(|a| matches!(a.artefact_kind, ArtefactKind::Url))
                .map(|a| a.content.as_str())
                .collect();
            assert!(urls.contains(&"ftp://downloads.example.com/tool.zip"));
        }

        #[test]
        fn canonicalizes_www_urls_to_http() {
            let data = b"www.example.com/path";
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
            let urls: Vec<&str> = out
                .iter()
                .filter(|a| matches!(a.artefact_kind, ArtefactKind::Url))
                .map(|a| a.content.as_str())
                .collect();
            assert!(urls.contains(&"http://www.example.com/path"));
        }

        #[test]
        fn canonicalizes_scheme_relative_urls() {
            let data = b"//example.com/index.html";
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
            let urls: Vec<&str> = out
                .iter()
                .filter(|a| matches!(a.artefact_kind, ArtefactKind::Url))
                .map(|a| a.content.as_str())
                .collect();
            assert!(urls.contains(&"http://example.com/index.html"));
        }

        #[test]
        fn accepts_single_label_host_urls() {
            let data = b"http://ntbld";
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
            let urls: Vec<&str> = out
                .iter()
                .filter(|a| matches!(a.artefact_kind, ArtefactKind::Url))
                .map(|a| a.content.as_str())
                .collect();
            assert!(urls.contains(&"http://ntbld"));
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
                    bitlocker_recovery_passwords: false,
                },
            );
            assert!(
                out.iter()
                    .all(|a| matches!(a.artefact_kind, ArtefactKind::Email))
            );
        }

        // Microsoft documents BitLocker recovery passwords as 8 groups of 6
        // decimal digits, each group divisible by 11 with quotient <= u16::MAX.
        // The fixtures below were constructed by choosing valid u16 quotients
        // and multiplying by 11 to produce real-shaped decoy passwords.
        const VALID_HYPHEN_BITLOCKER: &str =
            "111111-222222-333333-444444-555555-666666-072765-720885";
        const VALID_SPACE_BITLOCKER: &str =
            "111111 222222 333333 444444 555555 666666 072765 720885";

        #[test]
        fn extracts_hyphen_separated_bitlocker_recovery_password() {
            let mut data = Vec::from(b"prefix " as &[u8]);
            data.extend_from_slice(VALID_HYPHEN_BITLOCKER.as_bytes());
            data.extend_from_slice(b" suffix");
            let out = extract_artefacts("run1", 0, 0, 0, &data, ArtefactScanConfig::all());
            let hits: Vec<&str> = out
                .iter()
                .filter(|a| matches!(a.artefact_kind, ArtefactKind::BitlockerRecoveryPassword))
                .map(|a| a.content.as_str())
                .collect();
            assert_eq!(hits, vec![VALID_HYPHEN_BITLOCKER]);
        }

        #[test]
        fn extracts_whitespace_separated_bitlocker_recovery_password() {
            let mut data = Vec::from(b"key=" as &[u8]);
            data.extend_from_slice(VALID_SPACE_BITLOCKER.as_bytes());
            let out = extract_artefacts("run1", 0, 0, 0, &data, ArtefactScanConfig::all());
            let hits: Vec<&str> = out
                .iter()
                .filter(|a| matches!(a.artefact_kind, ArtefactKind::BitlockerRecoveryPassword))
                .map(|a| a.content.as_str())
                .collect();
            // Whitespace-separated input must be canonicalised to hyphenated form.
            assert_eq!(hits, vec![VALID_HYPHEN_BITLOCKER]);
        }

        #[test]
        fn bitlocker_utf16_offsets_use_source_byte_coordinates() {
            // Encode the recovery password as UTF-16LE so each ASCII character
            // occupies two source bytes. The recorded global_start/global_end
            // must reference the *source* byte range, not the decoded text.
            let mut data: Vec<u8> = Vec::new();
            for &b in VALID_HYPHEN_BITLOCKER.as_bytes() {
                data.push(b);
                data.push(0);
            }
            let chunk_start: u64 = 1_000;
            let local_start: u64 = 7;
            let out = extract_artefacts(
                "run1",
                chunk_start,
                local_start,
                flags::UTF16_LE,
                &data,
                ArtefactScanConfig::all(),
            );
            let hit = out
                .iter()
                .find(|a| matches!(a.artefact_kind, ArtefactKind::BitlockerRecoveryPassword))
                .expect("bitlocker hit");
            assert_eq!(hit.encoding, "utf-16le");
            assert_eq!(hit.global_start, chunk_start + local_start);
            // 55 ASCII chars × 2 bytes each = 110 source bytes; end is inclusive.
            assert_eq!(
                hit.global_end,
                chunk_start + local_start + (VALID_HYPHEN_BITLOCKER.len() as u64) * 2 - 1
            );
        }

        #[test]
        fn rejects_short_bitlocker_candidate() {
            let data = b"111111-222222-333333-444444-555555-666666-072765";
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
            assert!(
                out.iter()
                    .all(|a| !matches!(a.artefact_kind, ArtefactKind::BitlockerRecoveryPassword))
            );
        }

        #[test]
        fn rejects_long_bitlocker_candidate() {
            // 9 groups should be rejected: \b boundary fails after 8th group when
            // followed by another group of digits.
            let data = b"111111-222222-333333-444444-555555-666666-072765-720885-111111";
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
            assert!(
                out.iter()
                    .all(|a| !matches!(a.artefact_kind, ArtefactKind::BitlockerRecoveryPassword))
            );
        }

        #[test]
        fn rejects_long_bitlocker_candidate_whitespace() {
            // 9 whitespace-separated groups must also be rejected from both ends
            // (forward suppresses g1..g8; backward suppresses g2..g9).
            let data = b"111111 222222 333333 444444 555555 666666 072765 720885 111111";
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
            assert!(
                out.iter()
                    .all(|a| !matches!(a.artefact_kind, ArtefactKind::BitlockerRecoveryPassword))
            );
        }

        #[test]
        fn rejects_bitlocker_candidate_with_non_digits() {
            let data = b"11111A-222222-333333-444444-555555-666666-072765-720885";
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
            assert!(
                out.iter()
                    .all(|a| !matches!(a.artefact_kind, ArtefactKind::BitlockerRecoveryPassword))
            );
        }

        #[test]
        fn rejects_bitlocker_candidate_not_divisible_by_eleven() {
            // First group 111112 is not divisible by 11.
            let data = b"111112-222222-333333-444444-555555-666666-072765-720885";
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
            assert!(
                out.iter()
                    .all(|a| !matches!(a.artefact_kind, ArtefactKind::BitlockerRecoveryPassword))
            );
        }

        #[test]
        fn rejects_bitlocker_candidate_with_overflow_group() {
            // 720896 / 11 == 65536 which does not fit in u16.
            let data = b"111111-222222-333333-444444-555555-666666-072765-720896";
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
            assert!(
                out.iter()
                    .all(|a| !matches!(a.artefact_kind, ArtefactKind::BitlockerRecoveryPassword))
            );
        }

        #[test]
        fn rejects_bitlocker_candidate_with_mixed_separators() {
            let data = b"111111-222222 333333-444444-555555-666666-072765-720885";
            let out = extract_artefacts("run1", 0, 0, 0, data, ArtefactScanConfig::all());
            assert!(
                out.iter()
                    .all(|a| !matches!(a.artefact_kind, ArtefactKind::BitlockerRecoveryPassword))
            );
        }

        #[test]
        fn extracts_utf16le_bitlocker_recovery_password() {
            let mut data = Vec::new();
            for b in VALID_HYPHEN_BITLOCKER.as_bytes() {
                data.push(*b);
                data.push(0);
            }
            let out = extract_artefacts(
                "run1",
                0,
                0,
                flags::UTF16_LE,
                &data,
                ArtefactScanConfig::all(),
            );
            assert!(out.iter().any(|a| {
                matches!(a.artefact_kind, ArtefactKind::BitlockerRecoveryPassword)
                    && a.content == VALID_HYPHEN_BITLOCKER
                    && a.encoding == "utf-16le"
            }));
        }

        #[test]
        fn bitlocker_scan_disabled_by_config() {
            let mut data = Vec::from(b"prefix " as &[u8]);
            data.extend_from_slice(VALID_HYPHEN_BITLOCKER.as_bytes());
            let out = extract_artefacts(
                "run1",
                0,
                0,
                0,
                &data,
                ArtefactScanConfig {
                    urls: false,
                    emails: false,
                    phones: false,
                    bitlocker_recovery_passwords: false,
                },
            );
            assert!(
                out.iter()
                    .all(|a| !matches!(a.artefact_kind, ArtefactKind::BitlockerRecoveryPassword))
            );
        }
    }
}
