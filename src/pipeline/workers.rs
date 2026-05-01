//! # Pipeline Workers
//!
//! Worker thread spawning and management for the processing pipeline.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use tracing::{debug, warn};

use crate::carve::{CarveRegistry, ExtractionContext, PostCarveMetadata};
use crate::chunk::ScanChunk;
use crate::config::CarverLimits;
use crate::dedup::DedupTracker;
use crate::entropy;
use crate::evidence::EvidenceSource;
use crate::metadata::MetadataSink;
use crate::scanner::{NormalizedHit, SignatureScanner};
use crate::strings::artifacts::ArtefactScanConfig;
use crate::strings::{self, StringScanner, StringSpan};

use super::EntropyConfig;
use super::events::{EntropyShardEvent, FileShardEvent, MetadataEvent, StringShardEvent};
use super::limiter::CarveLimiter;

/// Job containing a chunk of data to scan
pub struct ScanJob {
    pub chunk: ScanChunk,
    pub data: Arc<Vec<u8>>,
}

/// Signature hit plus deterministic sequence assigned in evidence order.
pub struct HitJob {
    pub seq: u64,
    pub hit: NormalizedHit,
    permit: HitPermit,
}

/// Job containing string spans to process for artefacts
pub struct StringJob {
    pub chunk: ScanChunk,
    pub data: Arc<Vec<u8>>,
    pub spans: Vec<StringSpan>,
}

/// A validated carve result ready for I/O-bound disk writing.
/// Sent from validate (carve) workers to dedicated writer workers.
pub struct WriteJob {
    pub pending: crate::carve::PendingCarve,
    pub hit_global_offset: u64,
    pub should_discard: bool,
    pub carve_limiter: Arc<CarveLimiter>,
}

/// Completion event for one sequenced hit. `job == None` means the hit was
/// rejected before producing a pending carve.
pub struct CarveResult {
    pub seq: u64,
    pub job: Option<WriteJob>,
    _permit: HitPermit,
}

struct HitInFlightLimiter {
    state: Mutex<usize>,
    cond: Condvar,
    max: usize,
}

impl HitInFlightLimiter {
    fn new(max: usize) -> Self {
        debug_assert!(max > 0, "HitInFlightLimiter max must be > 0");
        Self {
            state: Mutex::new(0),
            cond: Condvar::new(),
            max,
        }
    }

    fn acquire(self: &Arc<Self>) -> HitPermit {
        let mut active = self.state.lock().expect("hit limiter lock poisoned");
        while *active >= self.max {
            active = self
                .cond
                .wait(active)
                .expect("hit limiter condvar poisoned");
        }
        *active += 1;
        HitPermit {
            limiter: self.clone(),
        }
    }

    fn release(&self) {
        let mut active = self.state.lock().expect("hit limiter lock poisoned");
        *active = active.saturating_sub(1);
        self.cond.notify_one();
    }
}

struct HitPermit {
    limiter: Arc<HitInFlightLimiter>,
}

impl Drop for HitPermit {
    fn drop(&mut self) {
        self.limiter.release();
    }
}

struct HitResultGuard<'a> {
    seq: u64,
    permit: Option<HitPermit>,
    write_tx: &'a Sender<CarveResult>,
    carve_errors: &'a AtomicU64,
}

impl<'a> HitResultGuard<'a> {
    fn new(
        seq: u64,
        permit: HitPermit,
        write_tx: &'a Sender<CarveResult>,
        carve_errors: &'a AtomicU64,
    ) -> Self {
        Self {
            seq,
            permit: Some(permit),
            write_tx,
            carve_errors,
        }
    }

    fn send_empty(&mut self) -> bool {
        let Some(permit) = self.permit.take() else {
            return true;
        };
        self.write_tx
            .send(CarveResult {
                seq: self.seq,
                job: None,
                _permit: permit,
            })
            .is_ok()
    }

    fn send_job(&mut self, job: WriteJob, hit_offset: u64) -> bool {
        let Some(permit) = self.permit.take() else {
            return true;
        };
        if let Err(err) = self.write_tx.send(CarveResult {
            seq: self.seq,
            job: Some(job),
            _permit: permit,
        }) {
            let result = err.into_inner();
            if let Some(job) = result.job {
                let _ = job.pending.discard();
            }
            self.carve_errors.fetch_add(1, Ordering::Relaxed);
            warn!("write channel closed while sending job at offset {hit_offset}");
            return false;
        }
        true
    }
}

impl Drop for HitResultGuard<'_> {
    fn drop(&mut self) {
        let Some(permit) = self.permit.take() else {
            return;
        };
        if std::thread::panicking() {
            self.carve_errors.fetch_add(1, Ordering::Relaxed);
            warn!(
                "carve worker panicked before emitting result for seq {}",
                self.seq
            );
        }
        let _ = self.write_tx.send(CarveResult {
            seq: self.seq,
            job: None,
            _permit: permit,
        });
    }
}

/// Spawn the metadata recording thread
pub fn spawn_metadata_thread(
    sink: Box<dyn MetadataSink>,
    rx: Receiver<MetadataEvent>,
    error_count: Arc<AtomicU64>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for event in rx {
            match event {
                MetadataEvent::File(file) => {
                    if let Err(err) = sink.record_file(&file) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::PostCarveMetadata(record) => {
                    record_post_carve_metadata(&*sink, record, &error_count, "metadata")
                }
                MetadataEvent::String(artefact) => {
                    if let Err(err) = sink.record_string(&artefact) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::History(record) => {
                    if let Err(err) = sink.record_history(&record) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::Cookie(record) => {
                    if let Err(err) = sink.record_cookie(&record) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::Download(record) => {
                    if let Err(err) = sink.record_download(&record) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::RunSummary(summary) => {
                    if let Err(err) = sink.record_run_summary(&summary) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::Entropy(region) => {
                    if let Err(err) = sink.record_entropy(&region) {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata record error: {err}");
                    }
                }
                MetadataEvent::Flush => {
                    if let Err(err) = sink.flush() {
                        error_count.fetch_add(1, Ordering::Relaxed);
                        warn!("metadata flush error: {err}");
                    }
                }
            }
        }
        // Final flush when channel closes
        if let Err(err) = sink.flush() {
            error_count.fetch_add(1, Ordering::Relaxed);
            warn!("metadata flush error: {err}");
        }
    })
}

/// Spawn the metadata router thread that dispatches events to per-shard channels.
///
/// Reads from the single producer channel and routes each event to the
/// appropriate shard channel. Flush events are broadcast to all shards.
pub fn spawn_metadata_router(
    rx: Receiver<MetadataEvent>,
    file_tx: Sender<FileShardEvent>,
    string_tx: Sender<StringShardEvent>,
    entropy_tx: Sender<EntropyShardEvent>,
    error_count: Arc<AtomicU64>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut dropped = 0u64;
        for event in rx {
            let ok = match event {
                MetadataEvent::File(f) => file_tx.send(FileShardEvent::File(f)).is_ok(),
                MetadataEvent::PostCarveMetadata(r) => {
                    file_tx.send(FileShardEvent::PostCarveMetadata(r)).is_ok()
                }
                MetadataEvent::String(s) => string_tx.send(StringShardEvent::String(s)).is_ok(),
                MetadataEvent::History(h) => file_tx.send(FileShardEvent::History(h)).is_ok(),
                MetadataEvent::Cookie(c) => file_tx.send(FileShardEvent::Cookie(c)).is_ok(),
                MetadataEvent::Download(d) => file_tx.send(FileShardEvent::Download(d)).is_ok(),
                MetadataEvent::RunSummary(s) => file_tx.send(FileShardEvent::RunSummary(s)).is_ok(),
                MetadataEvent::Entropy(e) => entropy_tx.send(EntropyShardEvent::Entropy(e)).is_ok(),
                MetadataEvent::Flush => {
                    if file_tx.send(FileShardEvent::Flush).is_err() {
                        dropped += 1;
                        error_count.fetch_add(1, Ordering::Relaxed);
                    }
                    if string_tx.send(StringShardEvent::Flush).is_err() {
                        dropped += 1;
                        error_count.fetch_add(1, Ordering::Relaxed);
                    }
                    if entropy_tx.send(EntropyShardEvent::Flush).is_err() {
                        dropped += 1;
                        error_count.fetch_add(1, Ordering::Relaxed);
                    }
                    continue;
                }
            };
            if !ok {
                dropped += 1;
                error_count.fetch_add(1, Ordering::Relaxed);
            }
        }
        if dropped > 0 {
            warn!("metadata router: {dropped} events dropped (shard channel disconnected)");
        }
        // Shard channels close when this function returns (senders are dropped),
        // causing shard threads to do their final flush and exit.
    })
}

/// Spawn a file-shard metadata writer thread.
pub fn spawn_file_shard_thread(
    sink: Box<dyn MetadataSink>,
    rx: Receiver<FileShardEvent>,
    error_count: Arc<AtomicU64>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for event in rx {
            let result = match event {
                FileShardEvent::File(file) => sink.record_file(&file),
                FileShardEvent::PostCarveMetadata(record) => {
                    write_post_carve_metadata(&*sink, record)
                }
                FileShardEvent::History(record) => sink.record_history(&record),
                FileShardEvent::Cookie(record) => sink.record_cookie(&record),
                FileShardEvent::Download(record) => sink.record_download(&record),
                FileShardEvent::RunSummary(summary) => sink.record_run_summary(&summary),
                FileShardEvent::Flush => sink.flush(),
            };
            if let Err(err) = result {
                error_count.fetch_add(1, Ordering::Relaxed);
                warn!("file shard metadata error: {err}");
            }
        }
        if let Err(err) = sink.flush() {
            error_count.fetch_add(1, Ordering::Relaxed);
            warn!("file shard final flush error: {err}");
        }
    })
}

fn record_post_carve_metadata(
    sink: &dyn MetadataSink,
    record: PostCarveMetadata,
    error_count: &AtomicU64,
    context: &str,
) {
    if let Err(err) = write_post_carve_metadata(sink, record) {
        error_count.fetch_add(1, Ordering::Relaxed);
        warn!("{context} record error: {err}");
    }
}

fn write_post_carve_metadata(
    sink: &dyn MetadataSink,
    record: PostCarveMetadata,
) -> Result<(), crate::metadata::MetadataError> {
    match record {
        PostCarveMetadata::BitlockerBek(record) => sink.record_bitlocker_bek(&record),
        PostCarveMetadata::WindowsArtefact(record) => sink.record_windows_artefact(&record),
    }
}

/// Spawn a string-shard metadata writer thread.
pub fn spawn_string_shard_thread(
    sink: Box<dyn MetadataSink>,
    rx: Receiver<StringShardEvent>,
    error_count: Arc<AtomicU64>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for event in rx {
            let result = match event {
                StringShardEvent::String(ref artefact) => sink.record_string(artefact),
                StringShardEvent::Flush => sink.flush(),
            };
            if let Err(err) = result {
                error_count.fetch_add(1, Ordering::Relaxed);
                warn!("string shard metadata error: {err}");
            }
        }
        if let Err(err) = sink.flush() {
            error_count.fetch_add(1, Ordering::Relaxed);
            warn!("string shard final flush error: {err}");
        }
    })
}

/// Spawn an entropy-shard metadata writer thread.
pub fn spawn_entropy_shard_thread(
    sink: Box<dyn MetadataSink>,
    rx: Receiver<EntropyShardEvent>,
    error_count: Arc<AtomicU64>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for event in rx {
            let result = match event {
                EntropyShardEvent::Entropy(ref region) => sink.record_entropy(region),
                EntropyShardEvent::Flush => sink.flush(),
            };
            if let Err(err) = result {
                error_count.fetch_add(1, Ordering::Relaxed);
                warn!("entropy shard metadata error: {err}");
            }
        }
        if let Err(err) = sink.flush() {
            error_count.fetch_add(1, Ordering::Relaxed);
            warn!("entropy shard final flush error: {err}");
        }
    })
}

/// Spawn signature scanning worker threads
#[allow(clippy::too_many_arguments)]
pub fn spawn_scan_workers(
    workers: usize,
    scanner: Arc<dyn SignatureScanner>,
    string_scanner: Option<Arc<dyn StringScanner>>,
    rx: Receiver<ScanJob>,
    fast_hit_tx: Sender<HitJob>,
    slow_hit_tx: Sender<HitJob>,
    carve_registry: Arc<CarveRegistry>,
    split_enabled: bool,
    string_tx: Option<Sender<StringJob>>,
    meta_tx: Sender<MetadataEvent>,
    run_id: String,
    entropy_cfg: Option<EntropyConfig>,
    hits_found: Arc<AtomicU64>,
    string_spans: Arc<AtomicU64>,
    sqlite_page_max_hits_per_chunk: usize,
    scan_time_ms: Arc<AtomicU64>,
    initial_chunk_id: u64,
    max_inflight_hits: usize,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();
    let worker_count = workers.max(1);
    let emit_order = Arc::new((Mutex::new(initial_chunk_id), Condvar::new()));
    let hit_sequence = Arc::new(AtomicU64::new(0));
    let hit_limiter = Arc::new(HitInFlightLimiter::new(max_inflight_hits.max(1)));

    for _ in 0..worker_count {
        let scanner = scanner.clone();
        let rx = rx.clone();
        let fast_hit_tx = fast_hit_tx.clone();
        let slow_hit_tx = slow_hit_tx.clone();
        let carve_registry = carve_registry.clone();
        let string_scanner = string_scanner.clone();
        let string_tx = string_tx.clone();
        let hits_found = hits_found.clone();
        let string_spans = string_spans.clone();
        let meta_tx = meta_tx.clone();
        let run_id = run_id.clone();
        let scan_time_ms = scan_time_ms.clone();
        let sqlite_page_max_hits_per_chunk = sqlite_page_max_hits_per_chunk.max(1);
        let emit_order = emit_order.clone();
        let hit_sequence = hit_sequence.clone();
        let hit_limiter = hit_limiter.clone();

        handles.push(thread::spawn(move || {
            for job in rx {
                let effective_valid = job.chunk.valid_length.min(job.data.len() as u64);
                let valid_len = effective_valid as usize;
                let mut sqlite_page_hits = 0usize;
                let mut sqlite_page_hits_dropped = 0usize;
                let chunk_id = job.chunk.id;

                // Scan for file signatures
                let scan_start = Instant::now();
                let scan_hits = scanner.scan_chunk(&job.chunk, &job.data);
                scan_time_ms.fetch_add(scan_start.elapsed().as_millis() as u64, Ordering::Relaxed);
                let mut normalized_hits = Vec::new();
                for hit in scan_hits {
                    if hit.local_offset >= effective_valid {
                        continue;
                    }
                    if hit.file_type_id == "sqlite_page" {
                        if sqlite_page_hits >= sqlite_page_max_hits_per_chunk {
                            sqlite_page_hits_dropped = sqlite_page_hits_dropped.saturating_add(1);
                            continue;
                        }
                        sqlite_page_hits = sqlite_page_hits.saturating_add(1);
                    }
                    let global_offset = job.chunk.start + hit.local_offset;
                    normalized_hits.push(NormalizedHit {
                        global_offset,
                        file_type_id: hit.file_type_id,
                        pattern_id: hit.pattern_id,
                        chunk_data: Some(Arc::clone(&job.data)),
                        chunk_start: job.chunk.start,
                    });
                }
                normalized_hits.sort_by(|a, b| {
                    a.global_offset
                        .cmp(&b.global_offset)
                        .then(a.file_type_id.cmp(&b.file_type_id))
                        .then(a.pattern_id.cmp(&b.pattern_id))
                });
                let mut hit_channel_closed = false;
                {
                    let (lock, cvar) = &*emit_order;
                    let mut next_chunk = lock.lock().expect("scan emit-order lock poisoned");
                    while *next_chunk != chunk_id {
                        next_chunk = cvar
                            .wait(next_chunk)
                            .expect("scan emit-order condvar poisoned");
                    }
                }
                // Leave next_chunk pointing at this chunk while emitting hits:
                // later chunks keep waiting, but permit acquisition and bounded
                // channel sends do not hold the emit-order mutex.
                for normalized in normalized_hits {
                    hits_found.fetch_add(1, Ordering::Relaxed);
                    let permit = hit_limiter.acquire();
                    let seq = hit_sequence.fetch_add(1, Ordering::Relaxed);
                    let file_type_id = normalized.file_type_id.clone();
                    // Classify hit as fast or slow based on carver metadata.
                    // When split is disabled (single worker mode), all hits go to slow.
                    let tx = if split_enabled && carve_registry.is_fast(&file_type_id) {
                        &fast_hit_tx
                    } else {
                        &slow_hit_tx
                    };
                    if let Err(err) = tx.send(HitJob {
                        seq,
                        hit: normalized,
                        permit,
                    }) {
                        warn!("hit channel closed while sending hit: {err}");
                        hit_channel_closed = true;
                        break;
                    }
                }
                {
                    let (lock, cvar) = &*emit_order;
                    let mut next_chunk = lock.lock().expect("scan emit-order lock poisoned");
                    debug_assert_eq!(*next_chunk, chunk_id);
                    if *next_chunk == chunk_id {
                        *next_chunk = next_chunk.saturating_add(1);
                    }
                    cvar.notify_all();
                }
                if sqlite_page_hits_dropped > 0 {
                    debug!(
                        "chunk {} sqlite_page hits capped: kept={} dropped={} cap={}",
                        chunk_id,
                        sqlite_page_hits,
                        sqlite_page_hits_dropped,
                        sqlite_page_max_hits_per_chunk
                    );
                }
                if hit_channel_closed {
                    break;
                }

                // Scan for strings if enabled
                if let (Some(scanner), Some(tx)) = (&string_scanner, &string_tx) {
                    let spans = scanner.scan_chunk(&job.chunk, &job.data);
                    if !spans.is_empty() {
                        let filtered: Vec<StringSpan> = spans
                            .into_iter()
                            .filter(|span| span.local_start < effective_valid)
                            .collect();
                        if !filtered.is_empty() {
                            string_spans.fetch_add(filtered.len() as u64, Ordering::Relaxed);
                            let string_job = StringJob {
                                chunk: job.chunk.clone(),
                                data: Arc::clone(&job.data),
                                spans: filtered,
                            };
                            if let Err(err) = tx.send(string_job) {
                                warn!("string channel closed while sending spans: {err}");
                                break;
                            }
                        }
                    }
                }

                // Detect high entropy regions if enabled
                if let Some(cfg) = entropy_cfg
                    && valid_len >= cfg.window_size
                {
                    let regions = entropy::detect_entropy_regions(
                        &run_id,
                        job.chunk.start,
                        &job.data[..valid_len],
                        cfg.window_size,
                        cfg.threshold,
                    );
                    for region in regions {
                        if let Err(err) = meta_tx.send(MetadataEvent::Entropy(region)) {
                            warn!("metadata channel closed while sending entropy region: {err}");
                            break;
                        }
                    }
                }
            }
        }));
    }

    handles
}

/// Per-file-type non-overlap accumulator used by the overlap arbiter.
///
/// Records committed closed `[start, end]` carve ranges and rejects any range
/// that intersects an already-accepted range for the same file type. Ranges are
/// keyed by final carved-file offsets, not signature-hit offsets, so this stays
/// correct even for carvers whose output start precedes the signature position.
/// Operates purely in the arbiter thread, so it does not need to be
/// `Send`/`Sync`-shared.
#[derive(Default)]
struct OverlapTracker {
    ranges_by_type: HashMap<String, BTreeMap<u64, u64>>,
}

impl OverlapTracker {
    fn new() -> Self {
        Self::default()
    }

    fn range_intersects(&self, file_type: &str, start: u64, end: u64) -> bool {
        let Some(ranges) = self.ranges_by_type.get(file_type) else {
            return false;
        };
        if let Some((_, prev_end)) = ranges.range(..=start).next_back()
            && *prev_end >= start
        {
            return true;
        }
        if let Some((next_start, _)) = ranges.range(start..).next()
            && *next_start <= end
        {
            return true;
        }
        false
    }

    fn record_non_overlapping(&mut self, file_type: &str, start: u64, end: u64) {
        debug_assert!(!self.range_intersects(file_type, start, end));
        self.ranges_by_type
            .entry(file_type.to_owned())
            .or_default()
            .insert(start, end);
    }

    /// Try to record `[start, end]` for `file_type`. Returns `true` if the
    /// range was accepted (no intersection with prior ranges), `false` if
    /// it was rejected.
    #[cfg(test)]
    fn try_commit(&mut self, file_type: &str, start: u64, end: u64) -> bool {
        if self.range_intersects(file_type, start, end) {
            return false;
        }
        self.record_non_overlapping(file_type, start, end);
        true
    }
}

/// Per-type concurrency semaphore map shared across slow carve workers.
pub type TypeSemaphores = Arc<HashMap<String, Arc<CountingSemaphore>>>;

/// Stable configuration shared by a carve worker pool.
#[derive(Clone)]
pub struct CarveWorkerConfig {
    pub workers: usize,
    pub registry: Arc<CarveRegistry>,
    pub evidence: Arc<dyn EvidenceSource>,
    pub run_id: String,
    pub run_output_dir: PathBuf,
    pub deferred_buffer_bytes: usize,
    pub metadata_only: bool,
    pub hash_config: crate::hash::HashConfig,
    pub dedup_tracker: Option<Arc<DedupTracker>>,
    pub skip_duplicates: bool,
    pub type_semaphores: Option<TypeSemaphores>,
}

/// Runtime plumbing for a carve worker pool invocation.
#[derive(Clone)]
pub struct CarveWorkerRuntime {
    pub rx: Receiver<HitJob>,
    /// Channel that completed [`CarveResult`]s are sent on. Carve workers send
    /// directly to the overlap arbiter (see [`spawn_overlap_arbiter`]),
    /// which deterministically deconflicts overlapping ranges before
    /// forwarding accepted jobs to the writer pool.
    pub write_tx: Sender<CarveResult>,
    pub carve_limiter: Arc<CarveLimiter>,
    pub carve_errors: Arc<AtomicU64>,
    pub carve_time_ms: Arc<AtomicU64>,
    pub files_rejected: Arc<AtomicU64>,
    pub files_prevalidation_rejected: Arc<AtomicU64>,
    pub duplicates_found: Arc<AtomicU64>,
}

/// A simple blocking counting semaphore for per-type concurrency limits.
pub struct CountingSemaphore {
    state: std::sync::Mutex<usize>,
    cond: std::sync::Condvar,
    max: usize,
}

impl CountingSemaphore {
    pub fn new(max: usize) -> Self {
        // Internal-only: callers (build_type_semaphores) filter max==0 before construction.
        debug_assert!(max > 0, "CountingSemaphore max must be > 0");
        assert!(max > 0, "CountingSemaphore max must be > 0");
        Self {
            state: std::sync::Mutex::new(0),
            cond: std::sync::Condvar::new(),
            max,
        }
    }

    /// Block until a permit is available, then return a guard that releases on drop.
    pub fn acquire(&self) -> SemaphoreGuard<'_> {
        // Poisoned mutex means a prior thread panicked; propagating is the safest choice.
        let mut active = self.state.lock().expect("semaphore lock poisoned");
        while *active >= self.max {
            active = self.cond.wait(active).expect("semaphore condvar poisoned");
        }
        *active += 1;
        SemaphoreGuard { sem: self }
    }

    fn release(&self) {
        let mut active = self.state.lock().expect("semaphore lock poisoned");
        *active = active.saturating_sub(1);
        self.cond.notify_one();
    }
}

pub struct SemaphoreGuard<'a> {
    sem: &'a CountingSemaphore,
}

impl Drop for SemaphoreGuard<'_> {
    fn drop(&mut self) {
        self.sem.release();
    }
}

/// Build per-type semaphores from carver_limits configuration.
pub fn build_type_semaphores(limits: &HashMap<String, CarverLimits>) -> TypeSemaphores {
    let mut map = HashMap::new();
    for (type_id, lim) in limits {
        if let Some(max) = lim.max_concurrent
            && max > 0
        {
            map.insert(type_id.clone(), Arc::new(CountingSemaphore::new(max)));
        }
    }
    Arc::new(map)
}

/// Spawn file carving worker threads
pub fn spawn_carve_workers(
    cfg: CarveWorkerConfig,
    runtime: CarveWorkerRuntime,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();
    let worker_count = cfg.workers.max(1);

    for _ in 0..worker_count {
        let registry = cfg.registry.clone();
        let evidence = cfg.evidence.clone();
        let run_id = cfg.run_id.clone();
        let run_output_dir = cfg.run_output_dir.clone();
        let rx = runtime.rx.clone();
        let write_tx = runtime.write_tx.clone();
        let carve_limiter = runtime.carve_limiter.clone();
        let carve_errors = runtime.carve_errors.clone();
        let carve_time_ms = runtime.carve_time_ms.clone();
        let files_rejected = runtime.files_rejected.clone();
        let files_prevalidation_rejected = runtime.files_prevalidation_rejected.clone();
        let hash_config = cfg.hash_config.clone();
        let dedup_tracker = cfg.dedup_tracker.clone();
        let duplicates_found = runtime.duplicates_found.clone();
        let type_semaphores = cfg.type_semaphores.clone();
        let deferred_buffer_bytes = cfg.deferred_buffer_bytes;
        let metadata_only = cfg.metadata_only;
        let skip_duplicates = cfg.skip_duplicates;

        handles.push(thread::spawn(move || {
            let carved_root = run_output_dir.join("carved");
            let mut ctx = ExtractionContext {
                run_id: &run_id,
                output_root: &carved_root,
                evidence: evidence.as_ref(),
                deferred_buffer_bytes,
                metadata_only,
                hash_config,
                io_buf: std::cell::RefCell::new(Vec::new()),
                chunk_data: None,
                chunk_start: 0,
            };

            for hit_job in rx {
                let seq = hit_job.seq;
                let hit = hit_job.hit;
                let mut result_guard =
                    HitResultGuard::new(seq, hit_job.permit, &write_tx, &carve_errors);
                ctx.chunk_data = hit.chunk_data.clone();
                ctx.chunk_start = hit.chunk_start;
                let handler = match registry.get(&hit.file_type_id) {
                    Some(handler) => handler,
                    None => {
                        debug!("no handler for file_type={}", hit.file_type_id);
                        if !result_guard.send_empty() {
                            break;
                        }
                        continue;
                    }
                };

                match handler.pre_validate(evidence.as_ref(), hit.global_offset) {
                    Ok(crate::carve::PreValidation::Proceed) => { /* continue to process_hit */ }
                    Ok(crate::carve::PreValidation::Reject(reason)) => {
                        files_prevalidation_rejected.fetch_add(1, Ordering::Relaxed);
                        debug!(
                            "pre_validate rejected {} at offset {}: {reason}",
                            hit.file_type_id, hit.global_offset
                        );
                        if !result_guard.send_empty() {
                            break;
                        }
                        continue;
                    }
                    Err(err) => {
                        carve_errors.fetch_add(1, Ordering::Relaxed);
                        warn!(
                            "pre_validate error for {} at offset {}: {err}",
                            hit.file_type_id, hit.global_offset
                        );
                        if !result_guard.send_empty() {
                            break;
                        }
                        continue;
                    }
                }

                // Per-type semaphore limits concurrent process_hit() calls only;
                // disk I/O is handled by the writer pool and is not type-limited.
                let _type_permit = type_semaphores.as_ref().and_then(
                    |sems: &Arc<HashMap<String, Arc<CountingSemaphore>>>| {
                        sems.get(hit.file_type_id.as_str()).map(|sem| sem.acquire())
                    },
                );

                let carve_start = Instant::now();
                let result = handler.process_hit(&hit, &ctx);
                carve_time_ms
                    .fetch_add(carve_start.elapsed().as_millis() as u64, Ordering::Relaxed);
                match result {
                    Ok(Some(mut pending)) => {
                        // Zero-write dedup: check BEFORE materializing file
                        let mut should_discard = false;
                        if let Some(ref tracker) = dedup_tracker
                            && let Some(ref sha) = pending.file.sha256
                        {
                            let dedup_result =
                                tracker.check_and_register(sha, pending.file.global_start);
                            if dedup_result.is_duplicate {
                                pending.file.is_duplicate = true;
                                pending.file.duplicate_of_offset = dedup_result.duplicate_of_offset;
                                duplicates_found.fetch_add(1, Ordering::Relaxed);
                                if skip_duplicates {
                                    should_discard = true;
                                }
                            }
                        }

                        let write_job = WriteJob {
                            pending,
                            hit_global_offset: hit.global_offset,
                            should_discard,
                            carve_limiter: carve_limiter.clone(),
                        };
                        // Drop per-type permit before send to avoid holding it
                        // during potential backpressure on the write channel.
                        drop(_type_permit);
                        if !result_guard.send_job(write_job, hit.global_offset) {
                            break;
                        }
                    }
                    Ok(None) => {
                        files_rejected.fetch_add(1, Ordering::Relaxed);
                        if !result_guard.send_empty() {
                            break;
                        }
                    }
                    Err(err) => {
                        carve_errors.fetch_add(1, Ordering::Relaxed);
                        warn!("carve error at offset {}: {err}", hit.global_offset);
                        if !result_guard.send_empty() {
                            break;
                        }
                    }
                }
            }
        }));
    }

    handles
}

/// Spawn the deterministic overlap arbiter thread.
///
/// The arbiter sits between the carve worker pool and the writer pool. It
/// receives one [`CarveResult`] for every sequenced hit and releases results
/// to the writer pool in deterministic evidence order:
///
/// 1. Scan workers sort each chunk's hits by `(global_offset, file_type, pattern_id)`
///    and assign monotonic sequences in that order while holding the per-chunk
///    emit-order turn.
/// 2. The arbiter waits for the next sequence and accepts each final carved range that does
///    not intersect any previously accepted range for the same file type.
///
/// Accepted jobs are forwarded to the writer pool in sorted order; rejected
/// jobs are discarded (their on-disk staging files are removed via
/// [`crate::carve::PendingCarve::discard`]) and counted in
/// `overlap_skipped`.
///
/// Sequenced hits carry an in-flight permit that is released only after the
/// arbiter processes the result, bounding out-of-order buffering and keeping
/// overlap decisions reproducible without end-of-input candidate storage.
pub fn spawn_overlap_arbiter(
    rx: Receiver<CarveResult>,
    write_tx: Sender<WriteJob>,
    overlap_skipped: Arc<AtomicU64>,
    files_capped: Arc<AtomicU64>,
    carve_errors: Arc<AtomicU64>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut accepted = OverlapTracker::new();
        let mut pending: BTreeMap<u64, CarveResult> = BTreeMap::new();
        let mut next_seq = 0u64;
        let mut write_closed = false;

        for result in rx {
            pending.insert(result.seq, result);
            while let Some(result) = pending.remove(&next_seq) {
                if let Some(job) = result.job {
                    arbitrate_write_job(
                        job,
                        &write_tx,
                        &mut accepted,
                        &overlap_skipped,
                        &files_capped,
                        &carve_errors,
                        &mut write_closed,
                    );
                }
                next_seq = next_seq.saturating_add(1);
            }
        }

        for (_, result) in pending {
            if let Some(job) = result.job {
                let _ = job.pending.discard();
            }
        }
    })
}

fn arbitrate_write_job(
    job: WriteJob,
    write_tx: &Sender<WriteJob>,
    accepted: &mut OverlapTracker,
    overlap_skipped: &AtomicU64,
    files_capped: &AtomicU64,
    carve_errors: &AtomicU64,
    write_closed: &mut bool,
) {
    if *write_closed {
        let _ = job.pending.discard();
        return;
    }

    let file_type = job.pending.file.file_type.clone();
    let start = job.pending.file.global_start;
    let end = job.pending.file.global_end;
    if accepted.range_intersects(&file_type, start, end) {
        overlap_skipped.fetch_add(1, Ordering::Relaxed);
        let hit_offset = job.hit_global_offset;
        let _ = job.pending.discard();
        debug!(
            "overlap arbiter rejected {} at offset {} ({}..={})",
            file_type, hit_offset, start, end
        );
        return;
    }

    if !job.carve_limiter.try_reserve() {
        files_capped.fetch_add(1, Ordering::Relaxed);
        let hit_offset = job.hit_global_offset;
        let _ = job.pending.discard();
        debug!(
            "max_files cap rejected {} at offset {} ({}..={})",
            file_type, hit_offset, start, end
        );
        return;
    }

    accepted.record_non_overlapping(&file_type, start, end);
    if let Err(err) = write_tx.send(job) {
        let job = err.into_inner();
        job.carve_limiter.release();
        let _ = job.pending.discard();
        carve_errors.fetch_add(1, Ordering::Relaxed);
        warn!("write channel closed while arbiter forwarding accepted job");
        *write_closed = true;
    }
}

/// Spawn dedicated I/O writer worker threads that flush/discard validated
/// carve results and emit metadata events.
pub fn spawn_write_workers(
    workers: usize,
    rx: Receiver<WriteJob>,
    meta_tx: Sender<MetadataEvent>,
    carve_errors: Arc<AtomicU64>,
    duplicates_skipped: Arc<AtomicU64>,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();
    let worker_count = workers.max(1);

    for _ in 0..worker_count {
        let rx = rx.clone();
        let meta_tx = meta_tx.clone();
        let carve_errors = carve_errors.clone();
        let duplicates_skipped = duplicates_skipped.clone();

        handles.push(thread::spawn(move || {
            for job in rx {
                let mut pending = job.pending;
                let sidecars = if job.should_discard {
                    Vec::new()
                } else {
                    std::mem::take(&mut pending.post_metadata)
                };
                let file = if job.should_discard {
                    duplicates_skipped.fetch_add(1, Ordering::Relaxed);
                    job.carve_limiter.commit();
                    pending.discard()
                } else {
                    match pending.flush() {
                        Ok(f) => {
                            job.carve_limiter.commit();
                            f
                        }
                        Err(err) => {
                            job.carve_limiter.release();
                            carve_errors.fetch_add(1, Ordering::Relaxed);
                            warn!("flush error at offset {}: {err}", job.hit_global_offset);
                            continue;
                        }
                    }
                };
                if let Err(err) = meta_tx.send(MetadataEvent::File(file)) {
                    warn!("metadata channel closed while sending carved file: {err}");
                }
                for record in sidecars {
                    if let Err(err) = meta_tx.send(MetadataEvent::PostCarveMetadata(record)) {
                        warn!("metadata channel closed while sending post-carve metadata: {err}");
                        break;
                    }
                }
            }
        }));
    }

    handles
}

/// Spawn string artefact extraction worker threads
pub fn spawn_string_workers(
    workers: usize,
    run_id: String,
    rx: Receiver<StringJob>,
    meta_tx: Sender<MetadataEvent>,
    artefacts_found: Arc<AtomicU64>,
    scan_cfg: ArtefactScanConfig,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();
    let worker_count = workers.max(1);

    for _ in 0..worker_count {
        let rx = rx.clone();
        let meta_tx = meta_tx.clone();
        let run_id = run_id.clone();
        let artefacts_found = artefacts_found.clone();

        handles.push(thread::spawn(move || {
            for job in rx {
                for span in job.spans {
                    let start = span.local_start as usize;
                    let end = start.saturating_add(span.length as usize);
                    if end > job.data.len() {
                        continue;
                    }
                    let slice = &job.data[start..end];
                    let artefacts = strings::artifacts::extract_artefacts(
                        &run_id,
                        job.chunk.start,
                        span.local_start,
                        span.flags,
                        slice,
                        scan_cfg,
                    );
                    artefacts_found.fetch_add(artefacts.len() as u64, Ordering::Relaxed);
                    for artefact in artefacts {
                        if let Err(err) = meta_tx.send(MetadataEvent::String(artefact)) {
                            warn!("metadata channel closed while sending string artefact: {err}");
                            break;
                        }
                    }
                }
            }
        }));
    }

    handles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_tracker_accepts_first_range_for_type() {
        let mut tracker = OverlapTracker::new();
        assert!(tracker.try_commit("jpeg", 1000, 2000));
    }

    #[test]
    fn overlap_tracker_rejects_intersecting_range() {
        let mut tracker = OverlapTracker::new();
        assert!(tracker.try_commit("sqlite_page", 4096, 8191));
        assert!(
            !tracker.try_commit("sqlite_page", 4608, 8703),
            "later-start range that intersects committed range must be rejected"
        );
    }

    #[test]
    fn overlap_tracker_rejects_range_enclosed_by_prior_range() {
        let mut tracker = OverlapTracker::new();
        assert!(tracker.try_commit("sqlite_page", 4096, 16383));
        assert!(
            !tracker.try_commit("sqlite_page", 8192, 12287),
            "enclosed range must be rejected when its containing range is already committed"
        );
    }

    #[test]
    fn overlap_tracker_accepts_adjacent_non_overlapping_ranges() {
        let mut tracker = OverlapTracker::new();
        assert!(tracker.try_commit("sqlite_page", 4096, 8191));
        assert!(
            tracker.try_commit("sqlite_page", 8192, 12287),
            "adjacent non-overlapping range must be accepted"
        );
    }

    #[test]
    fn overlap_tracker_rejects_shared_endpoint() {
        // global_end is inclusive: ranges sharing exactly one byte at the
        // boundary intersect and the second must be rejected. Guards
        // against an inadvertent change of `<=` to `<` in the interval
        // test.
        let mut tracker = OverlapTracker::new();
        assert!(tracker.try_commit("sqlite_page", 0, 8192));
        assert!(
            !tracker.try_commit("sqlite_page", 8192, 12288),
            "ranges sharing an endpoint are intersecting under closed-interval semantics"
        );
    }

    #[test]
    fn overlap_tracker_isolates_file_types() {
        let mut tracker = OverlapTracker::new();
        assert!(tracker.try_commit("sqlite_page", 4096, 8191));
        assert!(
            tracker.try_commit("jpeg", 4096, 8191),
            "different file types share no overlap state"
        );
    }

    /// Demonstrates the deterministic arbitration case where final ranges
    /// arrive in start order: the first range wins, overlapping later ranges
    /// are rejected, and non-overlapping later ranges are still accepted.
    /// This guards against the greedy-by-arrival regression flagged in the
    /// post-implementation review of issue #84 (e.g. the A=[100,199],
    /// B=[150,249], C=[220,319] case where arrival order could otherwise
    /// drop C).
    #[test]
    fn overlap_tracker_sorted_arbitration_keeps_non_conflicting_later_ranges() {
        let mut tracker = OverlapTracker::new();
        // Sorted by (start, end): A, B, C
        assert!(tracker.try_commit("sqlite_page", 100, 199), "A accepted");
        assert!(
            !tracker.try_commit("sqlite_page", 150, 249),
            "B rejected (overlaps A)"
        );
        assert!(
            tracker.try_commit("sqlite_page", 220, 319),
            "C accepted (does not overlap A)"
        );
    }

    #[test]
    fn overlap_tracker_handles_out_of_order_final_starts() {
        let mut tracker = OverlapTracker::new();
        assert!(tracker.try_commit("tar", 1000, 1099));
        assert!(
            tracker.try_commit("tar", 500, 599),
            "non-overlapping earlier final starts must still be accepted"
        );
        assert!(
            !tracker.try_commit("tar", 550, 650),
            "overlap with an earlier-start range must be rejected even when it arrived later"
        );
        assert!(
            !tracker.try_commit("tar", 900, 1000),
            "closed intervals sharing an endpoint must be rejected in out-of-order input"
        );
    }

    #[test]
    fn counting_semaphore_limits_concurrency() {
        let sem = CountingSemaphore::new(2);
        let _g1 = sem.acquire();
        let _g2 = sem.acquire();
        // Third acquire would block, but we can test state via try pattern:
        // Instead, test that releasing allows reacquire
        drop(_g1);
        let _g3 = sem.acquire(); // should not block
        drop(_g2);
        drop(_g3);
    }

    #[test]
    fn counting_semaphore_release_on_drop() {
        let sem = Arc::new(CountingSemaphore::new(1));
        {
            let _g = sem.acquire();
            // Guard held, active == 1
        }
        // Guard dropped, active == 0, should be able to acquire again
        let _g2 = sem.acquire();
    }

    #[test]
    fn build_type_semaphores_from_config() {
        let mut limits = HashMap::new();
        limits.insert(
            "sqlite".to_string(),
            CarverLimits {
                max_concurrent: Some(2),
            },
        );
        limits.insert(
            "mp3".to_string(),
            CarverLimits {
                max_concurrent: None,
            },
        );
        limits.insert(
            "pdf".to_string(),
            CarverLimits {
                max_concurrent: Some(0),
            },
        );
        let sems = build_type_semaphores(&limits);
        assert!(sems.contains_key("sqlite"));
        assert!(!sems.contains_key("mp3")); // None → no semaphore
        assert!(!sems.contains_key("pdf")); // max_concurrent=0 → no semaphore
    }
}
