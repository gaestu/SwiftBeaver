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
    #[serde(default = "default_parquet_row_group_size")]
    pub parquet_row_group_size: usize,
    #[serde(default)]
    pub enable_entropy_detection: bool,
    #[serde(default = "default_entropy_window_size")]
    pub entropy_window_size: usize,
    #[serde(default = "default_entropy_threshold")]
    pub entropy_threshold: f64,
    pub opencl_platform_index: Option<usize>,
    pub opencl_device_index: Option<usize>,
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
