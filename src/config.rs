use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sha2::{Digest, Sha256};

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
    #[serde(default)]
    pub enable_sqlite_page_recovery: bool,
    pub opencl_platform_index: Option<usize>,
    pub opencl_device_index: Option<usize>,
    #[serde(default)]
    pub zip_allowed_kinds: Option<Vec<String>>,
    pub file_types: Vec<FileTypeConfig>,
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: Config,
    pub config_hash: String,
}

pub fn load_config(path: Option<&Path>) -> Result<LoadedConfig> {
    let bytes: Vec<u8> = if let Some(p) = path {
        std::fs::read(p)?
    } else {
        include_bytes!("../config/default.yml").to_vec()
    };

    let mut config: Config = serde_yaml::from_slice(&bytes)?;
    if config.run_id.trim().is_empty() {
        config.run_id = generate_run_id();
    }

    let config_hash = hash_bytes(&bytes);

    Ok(LoadedConfig { config, config_hash })
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

fn default_entropy_window_size() -> usize {
    4096
}

fn default_entropy_threshold() -> f64 {
    7.5
}

fn default_true() -> bool {
    true
}

impl Config {
    /// Merge CLI options into the config.
    /// CLI flags override config file values.
    pub fn merge_cli(&mut self, cli: &crate::cli::CliOptions) {
        // String scanning
        if cli.scan_strings || cli.scan_utf16 || cli.scan_urls || cli.scan_emails || cli.scan_phones
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
        }
        if cli.no_scan_phones {
            self.enable_phone_scan = false;
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
        if cli.scan_entropy
            || cli.entropy_window_bytes.is_some()
            || cli.entropy_threshold.is_some()
        {
            self.enable_entropy_detection = true;
        }
        if let Some(window) = cli.entropy_window_bytes {
            self.entropy_window_size = window;
        }
        if let Some(threshold) = cli.entropy_threshold {
            self.entropy_threshold = threshold;
        }

        // SQLite page recovery
        if cli.scan_sqlite_pages {
            self.enable_sqlite_page_recovery = true;
        }
    }
}
