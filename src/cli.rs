use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum MetadataBackend {
    Jsonl,
    Csv,
    Parquet,
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

    #[test]
    fn parses_disable_zip_flag() {
        let opts = CliOptions::try_parse_from(["fastcarve", "--input", "image.dd", "--disable-zip"])
            .expect("parse");
        assert!(opts.disable_zip);
    }

    #[test]
    fn parses_utf16_flag() {
        let opts = CliOptions::try_parse_from(["fastcarve", "--input", "image.dd", "--scan-utf16"])
            .expect("parse");
        assert!(opts.scan_utf16);
    }

    #[test]
    fn parses_types_list() {
        let opts = CliOptions::try_parse_from([
            "fastcarve",
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
        let opts = CliOptions::try_parse_from(["fastcarve", "--input", "image.dd", "--scan-urls"])
            .expect("parse");
        assert!(opts.scan_urls);
        let opts =
            CliOptions::try_parse_from(["fastcarve", "--input", "image.dd", "--no-scan-urls"])
                .expect("parse");
        assert!(opts.no_scan_urls);
    }

    #[test]
    fn parses_entropy_flags() {
        let opts = CliOptions::try_parse_from([
            "fastcarve",
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
        let opts = CliOptions::try_parse_from([
            "fastcarve",
            "--input",
            "image.dd",
            "--scan-sqlite-pages",
        ])
        .expect("parse");
        assert!(opts.scan_sqlite_pages);
    }

    #[test]
    fn parses_limits() {
        let opts = CliOptions::try_parse_from([
            "fastcarve",
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
}
