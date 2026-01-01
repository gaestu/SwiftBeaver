use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

use fastcarve::{
    checkpoint, cli, config, constants::MIB, evidence, logging, metadata, pipeline, scanner,
    strings, util,
};

struct LoggingProgressReporter;

impl pipeline::ProgressReporter for LoggingProgressReporter {
    fn on_progress(&self, snapshot: &pipeline::ProgressSnapshot) {
        let percent = if snapshot.total_bytes > 0 {
            (snapshot.bytes_scanned as f64 / snapshot.total_bytes as f64) * 100.0
        } else {
            0.0
        };
        info!(
            "progress bytes_scanned={} total_bytes={} pct={:.1} hits={} files={} rate_mib={:.2} eta_secs={:?} carve_errs={} meta_errs={} sqlite_errs={}",
            snapshot.bytes_scanned,
            snapshot.total_bytes,
            percent,
            snapshot.hits_found,
            snapshot.files_carved,
            snapshot.throughput_mib,
            snapshot.eta_seconds,
            snapshot.carve_errors,
            snapshot.metadata_errors,
            snapshot.sqlite_errors
        );
    }
}

fn main() -> Result<()> {
    let cli_opts = cli::parse();
    logging::init_logging_with_format(cli_opts.log_format);
    let loaded = config::load_config(cli_opts.config_path.as_deref())?;
    let mut cfg = loaded.config;

    // Apply CLI overrides to config
    cfg.merge_cli(&cli_opts);

    // Apply file type filters
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

    util::apply_resource_limits(cfg.max_memory_mib, cfg.max_open_files)?;
    util::ensure_output_dir(&cli_opts.output)?;
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
        let hash = evidence::compute_sha256(evidence_source.as_ref(), 8 * MIB as usize)?;
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
        Some(Arc::from(strings::build_string_scanner(
            &cfg,
            cli_opts.gpu,
        )?))
    } else {
        None
    };

    let carve_registry = Arc::new(util::build_carve_registry(&cfg)?);

    let chunk_size = cli_opts.chunk_size_mib.saturating_mul(MIB);
    let overlap = cli_opts
        .overlap_kib
        .map(|kib| kib.saturating_mul(1024))
        .unwrap_or(cfg.overlap_bytes);

    let resume_state = match cli_opts.resume_from.as_ref() {
        Some(path) => Some(checkpoint::load_checkpoint(path).context("load checkpoint")?),
        None => None,
    };
    let checkpoint_path = cli_opts
        .checkpoint_path
        .clone()
        .or_else(|| cli_opts.resume_from.clone());
    let checkpoint_cfg = checkpoint_path.map(|path| pipeline::CheckpointConfig {
        path,
        resume: resume_state,
    });

    let cancel_flag = Arc::new(AtomicBool::new(false));
    {
        let cancel_flag = Arc::clone(&cancel_flag);
        ctrlc::set_handler(move || {
            cancel_flag.store(true, Ordering::Relaxed);
        })
        .context("failed to install Ctrl+C handler")?;
    }

    let progress = if cli_opts.progress_interval_secs == 0 {
        None
    } else {
        Some(pipeline::ProgressConfig {
            reporter: Arc::new(LoggingProgressReporter),
            interval: Duration::from_secs(cli_opts.progress_interval_secs),
        })
    };

    pipeline::run_pipeline_with_cancel(
        &cfg,
        evidence_source,
        sig_scanner,
        string_scanner,
        meta_sink,
        &run_output_dir,
        cli_opts.workers,
        chunk_size,
        overlap,
        cli_opts.max_bytes,
        cli_opts.max_chunks,
        carve_registry,
        cancel_flag,
        progress,
        checkpoint_cfg,
    )?;

    info!("fastcarve run finished");
    Ok(())
}
