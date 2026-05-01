use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::warn;

pub type ConfigResult<T> = std::result::Result<T, ConfigError>;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("phone region code cannot be empty")]
    EmptyPhoneRegion,
    #[error("unsupported phone region code: {region}")]
    UnsupportedPhoneRegion { region: String },
}

#[derive(Debug, Deserialize, Clone)]
pub struct FileTypeConfig {
    pub id: String,
    pub extensions: Vec<String>,
    pub header_patterns: Vec<PatternConfig>,
    pub footer_patterns: Vec<PatternConfig>,
    pub max_size: u64,
    pub min_size: u64,
    pub validator: String,
    #[serde(default)]
    pub require_eocd: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PatternConfig {
    pub id: String,
    pub hex: String,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum QuicktimeMode {
    Mov,
    Mp4,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PhoneMode {
    Off,
    Validated,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub run_id: String,
    pub overlap_bytes: u64,
    #[serde(default)]
    pub max_files: Option<u64>,
    #[serde(default)]
    pub max_memory_mib: Option<u64>,
    #[serde(default)]
    pub max_open_files: Option<u64>,
    pub enable_string_scan: bool,
    #[serde(default = "default_true")]
    pub enable_url_scan: bool,
    #[serde(default = "default_true")]
    pub enable_email_scan: bool,
    #[serde(default = "default_true")]
    pub enable_phone_scan: bool,
    #[serde(default)]
    pub phone_mode: Option<PhoneMode>,
    #[serde(default)]
    pub phone_default_region: Option<String>,
    #[serde(default)]
    pub phone_supported_regions: Vec<String>,
    #[serde(default = "default_true")]
    pub enable_bitlocker_recovery_scan: bool,
    #[serde(default)]
    pub string_scan_utf16: bool,
    #[serde(default = "default_string_min_len")]
    pub string_min_len: usize,
    #[serde(default = "default_string_max_len")]
    pub string_max_len: usize,
    #[serde(default = "default_gpu_max_hits")]
    pub gpu_max_hits_per_chunk: usize,
    #[serde(default = "default_gpu_max_string_spans")]
    pub gpu_max_string_spans_per_chunk: usize,
    #[serde(default = "default_parquet_row_group_size")]
    pub parquet_row_group_size: usize,
    #[serde(default)]
    pub enable_entropy_detection: bool,
    #[serde(default = "default_entropy_window_size")]
    pub entropy_window_size: usize,
    #[serde(default = "default_entropy_threshold")]
    pub entropy_threshold: f64,
    #[serde(default = "default_sqlite_page_max_hits_per_chunk")]
    pub sqlite_page_max_hits_per_chunk: usize,
    #[serde(default = "default_sqlite_wal_max_consecutive_checksum_failures")]
    pub sqlite_wal_max_consecutive_checksum_failures: u32,
    #[serde(default = "default_sqlite_max_consecutive_invalid_pages")]
    pub sqlite_max_consecutive_invalid_pages: u32,
    #[serde(default = "default_sqlite_min_valid_page_ratio")]
    pub sqlite_min_valid_page_ratio: f64,
    #[serde(default = "default_sqlite_suppress_wal_frame_lookback_frames")]
    pub sqlite_suppress_wal_frame_lookback_frames: u32,
    pub opencl_platform_index: Option<usize>,
    pub opencl_device_index: Option<usize>,
    #[serde(default)]
    pub zip_allowed_kinds: Option<Vec<String>>,
    #[serde(default)]
    pub ole_allowed_kinds: Option<Vec<String>>,
    #[serde(default = "default_quicktime_mode")]
    pub quicktime_mode: QuicktimeMode,
    #[serde(default = "default_deferred_buffer_kb")]
    pub deferred_buffer_kb: usize,
    #[serde(default = "default_ewf_cache_segments")]
    pub ewf_cache_segments: usize,
    #[serde(default = "default_ewf_reader_handles")]
    pub ewf_reader_handles: usize,
    #[serde(default = "default_hash_algorithms")]
    pub hash_algorithms: Vec<String>,
    #[serde(default)]
    pub enable_deduplication: bool,
    #[serde(default)]
    pub skip_duplicate_files: bool,
    #[serde(default = "default_fast_carve_worker_ratio")]
    pub fast_carve_worker_ratio: f64,
    #[serde(default = "default_write_workers")]
    pub write_workers: usize,
    /// Override for scan worker count (None = use CLI workers value)
    #[serde(default)]
    pub scan_workers: Option<usize>,
    /// Override for carve worker count (None = use CLI workers value)
    #[serde(default)]
    pub carve_workers: Option<usize>,
    #[serde(default)]
    pub carver_limits: HashMap<String, CarverLimits>,
    pub file_types: Vec<FileTypeConfig>,
}

/// Per-carver concurrency limits (optional, default: unlimited).
#[derive(Debug, Deserialize, Clone)]
pub struct CarverLimits {
    pub max_concurrent: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: Config,
    pub config_hash: String,
}

pub fn load_config(path: Option<&Path>) -> ConfigResult<LoadedConfig> {
    let bytes: Vec<u8> = if let Some(p) = path {
        std::fs::read(p)?
    } else {
        include_bytes!("../config/default.yml").to_vec()
    };

    let mut config: Config = serde_yaml::from_slice(&bytes)?;
    if config.run_id.trim().is_empty() {
        config.run_id = generate_run_id();
    }
    config.normalize_phone_config()?;

    let config_hash = hash_bytes(&bytes);

    Ok(LoadedConfig {
        config,
        config_hash,
    })
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    hex::encode(digest)
}

fn generate_run_id() -> String {
    let now = chrono::Utc::now();
    format!("{}_{}", now.format("%Y%m%dT%H%M%SZ"), rand_suffix())
}

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{:08x}", nanos)
}

fn default_string_min_len() -> usize {
    6
}

fn default_string_max_len() -> usize {
    1024
}

fn default_gpu_max_hits() -> usize {
    1_000_000
}

fn default_gpu_max_string_spans() -> usize {
    250_000
}

fn default_parquet_row_group_size() -> usize {
    10_000
}

fn default_quicktime_mode() -> QuicktimeMode {
    QuicktimeMode::Mov
}

fn default_entropy_window_size() -> usize {
    4096
}

fn default_entropy_threshold() -> f64 {
    7.5
}

fn default_sqlite_page_max_hits_per_chunk() -> usize {
    2048
}

fn default_sqlite_wal_max_consecutive_checksum_failures() -> u32 {
    2
}

fn default_sqlite_max_consecutive_invalid_pages() -> u32 {
    3
}

fn default_sqlite_min_valid_page_ratio() -> f64 {
    0.5
}

fn default_sqlite_suppress_wal_frame_lookback_frames() -> u32 {
    64
}

fn default_deferred_buffer_kb() -> usize {
    64
}

fn default_ewf_cache_segments() -> usize {
    4096
}

fn default_ewf_reader_handles() -> usize {
    0
}

fn default_hash_algorithms() -> Vec<String> {
    vec!["md5".to_string(), "sha256".to_string()]
}

fn default_true() -> bool {
    true
}

fn default_fast_carve_worker_ratio() -> f64 {
    crate::constants::DEFAULT_FAST_WORKER_RATIO
}

fn default_write_workers() -> usize {
    crate::constants::DEFAULT_WRITE_WORKERS
}

impl Config {
    pub fn effective_phone_mode(&self) -> PhoneMode {
        self.phone_mode
            .expect("normalize_phone_config sets phone_mode")
    }

    fn normalize_phone_config(&mut self) -> ConfigResult<()> {
        match self.phone_mode {
            Some(PhoneMode::Off) => self.enable_phone_scan = false,
            Some(PhoneMode::Validated) => self.enable_phone_scan = true,
            None => {
                self.phone_mode = Some(if self.enable_phone_scan {
                    PhoneMode::Validated
                } else {
                    PhoneMode::Off
                });
            }
        }

        self.phone_default_region =
            normalize_optional_region(self.phone_default_region.as_deref())?;

        let mut regions = Vec::with_capacity(self.phone_supported_regions.len());
        for region in &self.phone_supported_regions {
            regions.push(normalize_region(region)?);
        }
        regions.sort();
        regions.dedup();
        self.phone_supported_regions = regions;

        Ok(())
    }

    /// Merge CLI options into the config.
    /// CLI flags override config file values.
    pub fn merge_cli(&mut self, cli: &crate::cli::CliOptions) {
        // String scanning
        if cli.scan_strings
            || cli.scan_utf16
            || cli.scan_urls
            || cli.scan_emails
            || cli.scan_phones
            || cli.scan_bitlocker_recovery
        {
            self.enable_string_scan = true;
        }
        if cli.scan_utf16 {
            self.string_scan_utf16 = true;
        }

        // URL scanning
        if cli.scan_urls {
            self.enable_url_scan = true;
        }
        if cli.no_scan_urls {
            self.enable_url_scan = false;
        }

        // Email scanning
        if cli.scan_emails {
            self.enable_email_scan = true;
        }
        if cli.no_scan_emails {
            self.enable_email_scan = false;
        }

        // Phone scanning
        if cli.scan_phones {
            self.enable_phone_scan = true;
            self.phone_mode = Some(PhoneMode::Validated);
        }
        if cli.no_scan_phones {
            self.enable_phone_scan = false;
            self.phone_mode = Some(PhoneMode::Off);
        }

        // BitLocker recovery password scanning
        if cli.scan_bitlocker_recovery {
            self.enable_bitlocker_recovery_scan = true;
        }
        if cli.no_scan_bitlocker_recovery {
            self.enable_bitlocker_recovery_scan = false;
        }

        // String length
        if let Some(min_len) = cli.string_min_len {
            self.string_min_len = min_len;
        }

        // Output limits
        if let Some(max_files) = cli.max_files {
            self.max_files = Some(max_files);
        }
        if let Some(max_memory_mib) = cli.max_memory_mib {
            self.max_memory_mib = Some(max_memory_mib);
        }
        if let Some(max_open_files) = cli.max_open_files {
            self.max_open_files = Some(max_open_files);
        }

        // Entropy detection
        if cli.scan_entropy || cli.entropy_window_bytes.is_some() || cli.entropy_threshold.is_some()
        {
            self.enable_entropy_detection = true;
        }
        if let Some(window) = cli.entropy_window_bytes {
            self.entropy_window_size = window;
        }
        if let Some(threshold) = cli.entropy_threshold {
            self.entropy_threshold = threshold;
        }

        // Hash algorithms
        if let Some(ref algos) = cli.hash_algorithms {
            self.hash_algorithms = algos.clone();
        }

        // Deduplication
        if cli.dedupe {
            self.enable_deduplication = true;
        }
        if cli.skip_duplicates {
            self.skip_duplicate_files = true;
        }

        // Deduplication requires sha256 hashing
        if self.enable_deduplication
            && !self
                .hash_algorithms
                .iter()
                .any(|a| a.eq_ignore_ascii_case("sha256"))
        {
            warn!("deduplication requires sha256; adding sha256 to hash_algorithms");
            self.hash_algorithms.push("sha256".to_string());
        }

        // Validate skip_duplicate_files requires deduplication
        if self.skip_duplicate_files && !self.enable_deduplication {
            warn!(
                "skip_duplicate_files requires enable_deduplication; ignoring skip_duplicate_files"
            );
            self.skip_duplicate_files = false;
        }

        // Write workers
        if let Some(ww) = cli.write_workers {
            self.write_workers = ww;
        }
        if self.write_workers == 0 {
            warn!("write_workers must be >= 1; using 1");
            self.write_workers = 1;
        }

        // Scan workers
        if let Some(sw) = cli.scan_workers {
            self.scan_workers = Some(sw);
        }
        if let Some(ref mut sw) = self.scan_workers
            && *sw == 0
        {
            warn!("scan_workers must be >= 1; using 1");
            *sw = 1;
        }

        // Carve workers
        if let Some(cw) = cli.carve_workers {
            self.carve_workers = Some(cw);
        }
        if let Some(ref mut cw) = self.carve_workers
            && *cw == 0
        {
            warn!("carve_workers must be >= 1; using 1");
            *cw = 1;
        }
    }
}

fn normalize_optional_region(region: Option<&str>) -> ConfigResult<Option<String>> {
    region.map(normalize_region).transpose()
}

fn normalize_region(region: &str) -> ConfigResult<String> {
    let normalized = region.trim().to_ascii_uppercase();
    if normalized.is_empty() {
        return Err(ConfigError::EmptyPhoneRegion);
    }
    if normalized.parse::<phonenumber::country::Id>().is_err() {
        return Err(ConfigError::UnsupportedPhoneRegion {
            region: region.to_string(),
        });
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::{ConfigError, PhoneMode, load_config};
    use clap::Parser;

    #[test]
    fn legacy_enable_phone_scan_maps_to_validated_mode() {
        let mut cfg = load_config(None).expect("config").config;
        cfg.phone_mode = None;
        cfg.enable_phone_scan = true;
        cfg.normalize_phone_config()
            .expect("normalize phone config");
        assert_eq!(cfg.effective_phone_mode(), PhoneMode::Validated);
        assert!(cfg.enable_phone_scan);

        cfg.phone_mode = None;
        cfg.enable_phone_scan = false;
        cfg.normalize_phone_config()
            .expect("normalize phone config");
        assert_eq!(cfg.effective_phone_mode(), PhoneMode::Off);
        assert!(!cfg.enable_phone_scan);
    }

    #[test]
    fn cli_phone_flags_set_effective_phone_mode() {
        let mut cfg = load_config(None).expect("config").config;
        cfg.enable_phone_scan = false;
        cfg.phone_mode = Some(PhoneMode::Off);
        let scan_cli = crate::cli::CliOptions::parse_from([
            "swiftbeaver",
            "--input",
            "evidence.dd",
            "--scan-phones",
        ]);
        cfg.merge_cli(&scan_cli);
        assert_eq!(cfg.effective_phone_mode(), PhoneMode::Validated);
        assert!(cfg.enable_phone_scan);

        let no_scan_cli = crate::cli::CliOptions::parse_from([
            "swiftbeaver",
            "--input",
            "evidence.dd",
            "--no-scan-phones",
        ]);
        cfg.merge_cli(&no_scan_cli);
        assert_eq!(cfg.effective_phone_mode(), PhoneMode::Off);
        assert!(!cfg.enable_phone_scan);
    }

    #[test]
    fn invalid_phone_region_is_rejected() {
        let mut cfg = load_config(None).expect("config").config;
        cfg.phone_default_region = Some("XX".to_string());
        let err = cfg
            .normalize_phone_config()
            .expect_err("invalid region should fail");
        assert!(matches!(
            err,
            ConfigError::UnsupportedPhoneRegion { ref region } if region == "XX"
        ));
    }
}
