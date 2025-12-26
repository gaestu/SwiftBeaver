use std::sync::Arc;

use anyhow::Result;
use tracing::info;

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
    if cli_opts.scan_strings {
        cfg.enable_string_scan = true;
    }
    if let Some(min_len) = cli_opts.string_min_len {
        cfg.string_min_len = min_len;
    }
    if cli_opts.disable_zip {
        cfg.file_types.retain(|ft| ft.id != "zip");
        info!("zip carving disabled by CLI");
    }

    let run_output_dir = cli_opts.output.join(&cfg.run_id);
    std::fs::create_dir_all(&run_output_dir)?;

    let tool_version = env!("CARGO_PKG_VERSION");
    let evidence_path = cli_opts.input.clone();
    let evidence_sha256 = "".to_string();

    info!(
        "starting run_id={} input={} output={} workers={} chunk_mib={}",
        cfg.run_id,
        cli_opts.input.display(),
        run_output_dir.display(),
        cli_opts.workers,
        cli_opts.chunk_size_mib
    );

    let evidence_source = evidence::open_source(&cli_opts)?;
    let evidence_source = Arc::from(evidence_source);

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
