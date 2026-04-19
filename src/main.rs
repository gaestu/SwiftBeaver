use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

use swiftbeaver::{
    checkpoint, cli, config, constants::MIB, evidence, logging, metadata, pipeline, scanner,
    strings, util,
};

struct LoggingProgressReporter;

impl pipeline::ProgressReporter for LoggingProgressReporter {
    fn on_progress(&self, snapshot: &pipeline::ProgressSnapshot) {
        let eta_str = snapshot
            .eta_seconds
            .map(|s| format!("{:.0}s", s))
            .unwrap_or_else(|| "N/A".to_string());
        info!(
            "progress {:.1}% scanned={}/{} hits={} files={} rate={:.2}MiB/s eta={} errs=[carve:{} meta:{} sql:{}]",
            snapshot.completion_pct,
            snapshot.bytes_scanned,
            snapshot.total_bytes,
            snapshot.hits_found,
            snapshot.files_carved,
            snapshot.throughput_mib,
            eta_str,
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

    // Apply file type filters (support both --types and --enable-types)
    let types_filter = cli::get_types_filter(&cli_opts);
    let unknown_types = util::filter_file_types(
        &mut cfg,
        types_filter.map(|v| v.as_slice()),
        cli_opts.disable_zip,
    );
    for unknown in unknown_types {
        warn!("unknown file type in --types/--enable-types: {unknown}");
    }
    if cli_opts.disable_zip {
        info!("zip carving disabled by CLI");
    }
    if types_filter.is_some() && cfg.file_types.is_empty() {
        warn!("no file types enabled after applying type filter");
    }
    if cli_opts.dry_run {
        info!("dry-run mode enabled: no files will be written");
    }
    if cli_opts.metadata_only {
        info!("metadata-only mode enabled: carved files will not be written to disk");
    }
    if cli_opts.validate_carved {
        info!("post-carving validation enabled");
    }
    if cfg.enable_string_scan
        && !cfg.enable_url_scan
        && !cfg.enable_email_scan
        && !cfg.enable_phone_scan
    {
        warn!("string scanning enabled but all artefact types are disabled");
    }

    util::apply_resource_limits(cfg.max_memory_mib, cfg.max_open_files)?;

    // In dry-run mode, skip output directory creation
    if !cli_opts.dry_run {
        util::ensure_output_dir(&cli_opts.output)?;
    }
    let run_output_dir = cli_opts.output.join(&cfg.run_id);
    if !cli_opts.dry_run {
        std::fs::create_dir_all(&run_output_dir)?;
    }

    let tool_version = env!("CARGO_PKG_VERSION");
    let evidence_path = cli_opts.input.clone();

    let scan_workers = cfg.scan_workers.unwrap_or(cli_opts.workers);
    let carve_workers = cfg.carve_workers.unwrap_or(cli_opts.workers);

    info!(
        "starting run_id={} input={} output={} scan_workers={} carve_workers={} chunk_mib={}",
        cfg.run_id,
        cli_opts.input.display(),
        run_output_dir.display(),
        scan_workers,
        carve_workers,
        cli_opts.chunk_size_mib
    );

    let ewf_reader_handles = if cfg.ewf_reader_handles == 0 {
        (num_cpus::get() / 4).clamp(2, 4)
    } else {
        let clamped = cfg.ewf_reader_handles.clamp(1, 32);
        if clamped != cfg.ewf_reader_handles {
            warn!(
                "ewf_reader_handles {} clamped to {}",
                cfg.ewf_reader_handles, clamped
            );
        }
        clamped
    };
    info!("EWF reader handles: {ewf_reader_handles}");

    let evidence_source = evidence::open_source(&cli_opts, ewf_reader_handles)?;
    let evidence_source = evidence::wrap_with_cache(evidence_source, cfg.ewf_cache_segments);
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
    let meta_sink: Box<dyn metadata::MetadataSink> = if cli_opts.dry_run {
        metadata::build_dry_run_sink()
    } else {
        metadata::build_sink(
            meta_backend,
            &cfg,
            &cfg.run_id,
            tool_version,
            &loaded.config_hash,
            &evidence_path,
            &evidence_sha256,
            &run_output_dir,
        )?
    };

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

    let carve_registry = Arc::new(util::build_carve_registry(&cfg, cli_opts.dry_run)?);

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
        scan_workers,
        carve_workers,
        chunk_size,
        overlap,
        cli_opts.max_bytes,
        cli_opts.max_chunks,
        carve_registry,
        cancel_flag,
        progress,
        checkpoint_cfg,
        cli_opts.metadata_only,
    )?;

    info!("SwiftBeaver run finished");
    Ok(())
}
