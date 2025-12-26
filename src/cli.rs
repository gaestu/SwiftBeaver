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

    /// Override minimum string length when scanning
    #[arg(long)]
    pub string_min_len: Option<usize>,

    /// Disable ZIP carving (skips zip/docx/xlsx/pptx)
    #[arg(long)]
    pub disable_zip: bool,
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
}
