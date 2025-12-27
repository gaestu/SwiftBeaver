use std::sync::Arc;

use anyhow::{bail, Result};
use tracing::{info, warn};

use fastcarve::{
    cli,
    config,
    evidence,
    logging,
    metadata,
    scanner,
    strings,
    util,
};

fn main() -> Result<()> {
    logging::init_logging();

    let cli_opts = cli::parse();
    let loaded = config::load_config(cli_opts.config_path.as_deref())?;
    let mut cfg = loaded.config;
    if cli_opts.scan_strings
        || cli_opts.scan_utf16
        || cli_opts.scan_urls
        || cli_opts.scan_emails
        || cli_opts.scan_phones
    {
        cfg.enable_string_scan = true;
    }
    if cli_opts.scan_utf16 {
        cfg.string_scan_utf16 = true;
    }
    if cli_opts.scan_urls {
        cfg.enable_url_scan = true;
    }
    if cli_opts.no_scan_urls {
        cfg.enable_url_scan = false;
    }
    if cli_opts.scan_emails {
        cfg.enable_email_scan = true;
    }
    if cli_opts.no_scan_emails {
        cfg.enable_email_scan = false;
    }
    if cli_opts.scan_phones {
        cfg.enable_phone_scan = true;
    }
    if cli_opts.no_scan_phones {
        cfg.enable_phone_scan = false;
    }
    if let Some(min_len) = cli_opts.string_min_len {
        cfg.string_min_len = min_len;
    }
    if cli_opts.scan_entropy
        || cli_opts.entropy_window_bytes.is_some()
        || cli_opts.entropy_threshold.is_some()
    {
        cfg.enable_entropy_detection = true;
    }
    if let Some(window) = cli_opts.entropy_window_bytes {
        cfg.entropy_window_size = window;
    }
    if let Some(threshold) = cli_opts.entropy_threshold {
        cfg.entropy_threshold = threshold;
    }
    let unknown_types =
        util::filter_file_types(&mut cfg, cli_opts.types.as_deref(), cli_opts.disable_zip);
    for unknown in unknown_types {
        warn!("unknown file type in --types: {unknown}");
    }
    if cli_opts.disable_zip {
        info!("zip carving disabled by CLI");
    }
    if cli_opts.types.is_some() && cfg.file_types.is_empty() {
        warn!("no file types enabled after applying --types filter");
    }
    if cfg.enable_string_scan
        && !cfg.enable_url_scan
        && !cfg.enable_email_scan
        && !cfg.enable_phone_scan
    {
        warn!("string scanning enabled but all artefact types are disabled");
    }

    let run_output_dir = cli_opts.output.join(&cfg.run_id);
    std::fs::create_dir_all(&run_output_dir)?;

    let tool_version = env!("CARGO_PKG_VERSION");
    let evidence_path = cli_opts.input.clone();

    info!(
        "starting run_id={} input={} output={} workers={} chunk_mib={}",
        cfg.run_id,
        cli_opts.input.display(),
        run_output_dir.display(),
        cli_opts.workers,
        cli_opts.chunk_size_mib
    );

    let evidence_source = evidence::open_source(&cli_opts)?;
    let evidence_source: Arc<dyn evidence::EvidenceSource> = Arc::from(evidence_source);

    if cli_opts.evidence_sha256.is_some() && cli_opts.compute_evidence_sha256 {
        bail!("set either --evidence-sha256 or --compute-evidence-sha256, not both");
    }

    let evidence_sha256 = if let Some(hash) = cli_opts.evidence_sha256.as_ref() {
        hash.trim().to_string()
    } else if cli_opts.compute_evidence_sha256 {
        info!("computing evidence sha256 (full pass)");
        let hash = evidence::compute_sha256(evidence_source.as_ref(), 8 * 1024 * 1024)?;
        info!("evidence sha256={hash}");
        hash
    } else {
        String::new()
    };

    let meta_backend = util::backend_from_cli(cli_opts.metadata_backend);
    let meta_sink = metadata::build_sink(
        meta_backend,
        &cfg,
        &cfg.run_id,
        tool_version,
        &loaded.config_hash,
        &evidence_path,
        &evidence_sha256,
        &run_output_dir,
    )?;

    let sig_scanner = scanner::build_signature_scanner(&cfg, cli_opts.gpu)?;
    let sig_scanner = Arc::from(sig_scanner);

    let string_scanner = if cfg.enable_string_scan {
        Some(Arc::from(strings::build_string_scanner(&cfg, cli_opts.gpu)?))
    } else {
        None
    };

    let chunk_size = cli_opts.chunk_size_mib.saturating_mul(1024 * 1024);
    let overlap = cli_opts
        .overlap_kib
        .map(|kib| kib.saturating_mul(1024))
        .unwrap_or(cfg.overlap_bytes);

    util::run_pipeline(
        &cfg,
        evidence_source,
        sig_scanner,
        string_scanner,
        meta_sink,
        &run_output_dir,
        cli_opts.workers,
        chunk_size,
        overlap,
    )?;

    info!("fastcarve run finished");
    Ok(())
}
