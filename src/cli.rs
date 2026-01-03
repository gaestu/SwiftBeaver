use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum MetadataBackend {
    Jsonl,
    Csv,
    Parquet,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum LogFormat {
    Text,
    Json,
}

#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct CliOptions {
    /// Input image (raw, E01, or device)
    #[arg(short, long)]
    pub input: PathBuf,

    /// Output directory for carved files and metadata
    #[arg(short, long, default_value = "./output")]
    pub output: PathBuf,

    /// Optional path to config file (YAML)
    #[arg(long)]
    pub config_path: Option<PathBuf>,

    /// Enable GPU acceleration if available
    #[arg(long)]
    pub gpu: bool,

    /// Number of worker threads
    #[arg(long, default_value_t = num_cpus::get())]
    pub workers: usize,

    /// Chunk size, in MiB
    #[arg(long, default_value_t = 512)]
    pub chunk_size_mib: u64,

    /// Chunk overlap, in KiB (overrides config when set)
    #[arg(long)]
    pub overlap_kib: Option<u64>,

    /// Metadata backend
    #[arg(long, value_enum, default_value_t = MetadataBackend::Jsonl)]
    pub metadata_backend: MetadataBackend,

    /// Log format
    #[arg(long, value_enum, default_value_t = LogFormat::Text)]
    pub log_format: LogFormat,

    /// Progress log interval in seconds (0 disables progress logging)
    #[arg(long, default_value_t = 5)]
    pub progress_interval_secs: u64,

    /// Enable printable string scanning
    #[arg(long)]
    pub scan_strings: bool,

    /// Enable UTF-16 (LE/BE) string scanning
    #[arg(long)]
    pub scan_utf16: bool,

    /// Enable URL extraction from string spans
    #[arg(long, conflicts_with = "no_scan_urls")]
    pub scan_urls: bool,

    /// Disable URL extraction from string spans
    #[arg(long, conflicts_with = "scan_urls")]
    pub no_scan_urls: bool,

    /// Enable email extraction from string spans
    #[arg(long, conflicts_with = "no_scan_emails")]
    pub scan_emails: bool,

    /// Disable email extraction from string spans
    #[arg(long, conflicts_with = "scan_emails")]
    pub no_scan_emails: bool,

    /// Enable phone extraction from string spans
    #[arg(long, conflicts_with = "no_scan_phones")]
    pub scan_phones: bool,

    /// Disable phone extraction from string spans
    #[arg(long, conflicts_with = "scan_phones")]
    pub no_scan_phones: bool,

    /// Override minimum string length when scanning
    #[arg(long)]
    pub string_min_len: Option<usize>,

    /// Enable entropy-based region detection
    #[arg(long)]
    pub scan_entropy: bool,

    /// Entropy window size in bytes
    #[arg(long)]
    pub entropy_window_bytes: Option<usize>,

    /// Entropy threshold for high-entropy regions
    #[arg(long)]
    pub entropy_threshold: Option<f64>,

    /// Enable SQLite page-level URL recovery when DB parsing fails
    #[arg(long)]
    pub scan_sqlite_pages: bool,

    /// Stop after scanning this many bytes (approximate limit)
    #[arg(long)]
    pub max_bytes: Option<u64>,

    /// Stop after scanning this many chunks
    #[arg(long)]
    pub max_chunks: Option<u64>,

    /// Stop after carving this many files
    #[arg(long)]
    pub max_files: Option<u64>,

    /// Limit address space usage in MiB (Unix only)
    #[arg(long)]
    pub max_memory_mib: Option<u64>,

    /// Limit maximum open file descriptors (Unix only)
    #[arg(long)]
    pub max_open_files: Option<u64>,

    /// Write checkpoint state to this path on early exit
    #[arg(long)]
    pub checkpoint_path: Option<PathBuf>,

    /// Resume scanning from a checkpoint file
    #[arg(long)]
    pub resume_from: Option<PathBuf>,

    /// Provide evidence SHA-256 (hex) for metadata output
    #[arg(long)]
    pub evidence_sha256: Option<String>,

    /// Compute evidence SHA-256 before scanning (extra full pass)
    #[arg(long)]
    pub compute_evidence_sha256: bool,

    /// Disable ZIP carving (skips zip/docx/xlsx/pptx)
    #[arg(long)]
    pub disable_zip: bool,

    /// Limit carving to these file types (comma-separated list)
    #[arg(long, value_delimiter = ',')]
    pub types: Option<Vec<String>>,
}

pub fn parse() -> CliOptions {
    CliOptions::parse()
}

#[cfg(test)]
mod tests {
    use super::CliOptions;
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn parses_disable_zip_flag() {
        let opts =
            CliOptions::try_parse_from(["SwiftBeaver", "--input", "image.dd", "--disable-zip"])
                .expect("parse");
        assert!(opts.disable_zip);
    }

    #[test]
    fn parses_utf16_flag() {
        let opts = CliOptions::try_parse_from(["SwiftBeaver", "--input", "image.dd", "--scan-utf16"])
            .expect("parse");
        assert!(opts.scan_utf16);
    }

    #[test]
    fn parses_types_list() {
        let opts = CliOptions::try_parse_from([
            "SwiftBeaver",
            "--input",
            "image.dd",
            "--types",
            "jpeg,png,sqlite",
        ])
        .expect("parse");
        let types = opts.types.expect("types");
        assert_eq!(types, vec!["jpeg", "png", "sqlite"]);
    }

    #[test]
    fn parses_scan_url_flags() {
        let opts = CliOptions::try_parse_from(["SwiftBeaver", "--input", "image.dd", "--scan-urls"])
            .expect("parse");
        assert!(opts.scan_urls);
        let opts =
            CliOptions::try_parse_from(["SwiftBeaver", "--input", "image.dd", "--no-scan-urls"])
                .expect("parse");
        assert!(opts.no_scan_urls);
    }

    #[test]
    fn parses_entropy_flags() {
        let opts = CliOptions::try_parse_from([
            "SwiftBeaver",
            "--input",
            "image.dd",
            "--scan-entropy",
            "--entropy-window-bytes",
            "2048",
            "--entropy-threshold",
            "7.2",
        ])
        .expect("parse");
        assert!(opts.scan_entropy);
        assert_eq!(opts.entropy_window_bytes, Some(2048));
        assert_eq!(opts.entropy_threshold, Some(7.2));
    }

    #[test]
    fn parses_sqlite_page_flag() {
        let opts =
            CliOptions::try_parse_from(["SwiftBeaver", "--input", "image.dd", "--scan-sqlite-pages"])
                .expect("parse");
        assert!(opts.scan_sqlite_pages);
    }

    #[test]
    fn parses_limits() {
        let opts = CliOptions::try_parse_from([
            "SwiftBeaver",
            "--input",
            "image.dd",
            "--max-bytes",
            "1048576",
            "--max-chunks",
            "4",
        ])
        .expect("parse");
        assert_eq!(opts.max_bytes, Some(1_048_576));
        assert_eq!(opts.max_chunks, Some(4));
    }

    #[test]
    fn parses_log_format() {
        let opts = CliOptions::try_parse_from([
            "SwiftBeaver",
            "--input",
            "image.dd",
            "--log-format",
            "json",
        ])
        .expect("parse");
        assert!(matches!(opts.log_format, super::LogFormat::Json));
    }

    #[test]
    fn parses_progress_interval() {
        let opts = CliOptions::try_parse_from([
            "SwiftBeaver",
            "--input",
            "image.dd",
            "--progress-interval-secs",
            "10",
        ])
        .expect("parse");
        assert_eq!(opts.progress_interval_secs, 10);
    }

    #[test]
    fn parses_max_files() {
        let opts =
            CliOptions::try_parse_from(["SwiftBeaver", "--input", "image.dd", "--max-files", "25"])
                .expect("parse");
        assert_eq!(opts.max_files, Some(25));
    }

    #[test]
    fn parses_resource_limits() {
        let opts = CliOptions::try_parse_from([
            "SwiftBeaver",
            "--input",
            "image.dd",
            "--max-memory-mib",
            "256",
            "--max-open-files",
            "2048",
        ])
        .expect("parse");
        assert_eq!(opts.max_memory_mib, Some(256));
        assert_eq!(opts.max_open_files, Some(2048));
    }

    #[test]
    fn parses_checkpoint_paths() {
        let opts = CliOptions::try_parse_from([
            "SwiftBeaver",
            "--input",
            "image.dd",
            "--checkpoint-path",
            "checkpoint.json",
            "--resume-from",
            "resume.json",
        ])
        .expect("parse");
        assert_eq!(opts.checkpoint_path, Some(PathBuf::from("checkpoint.json")));
        assert_eq!(opts.resume_from, Some(PathBuf::from("resume.json")));
    }
}
