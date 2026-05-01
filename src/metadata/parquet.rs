use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arrow_array::builder::{
    BinaryBuilder, BooleanBuilder, Int32Builder, Int64Builder, StringBuilder,
    TimestampMicrosecondBuilder,
};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use crate::carve::CarvedFile;
use crate::carve::windows::WindowsArtefactRecord;
use crate::config::Config;
use crate::metadata::windows::flatten_windows_artefact;
use crate::metadata::{MetadataError, MetadataSink, RunSummary};
use crate::parsers::browser::{BrowserCookieRecord, BrowserDownloadRecord, BrowserHistoryRecord};
use crate::strings::artifacts::{ArtefactKind, StringArtefact};

#[derive(Clone)]
struct ParquetContext {
    run_id: String,
    tool_version: String,
    config_hash: String,
    evidence_path: String,
    evidence_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParquetCategory {
    FilesJpeg,
    FilesPng,
    FilesGif,
    FilesSqlite,
    FilesPdf,
    FilesZip,
    FilesWebp,
    FilesOther,
    ArtefactsUrls,
    ArtefactsEmails,
    ArtefactsPhones,
    ArtefactsBitlockerRecoveryPasswords,
    BrowserHistory,
    BrowserCookies,
    BrowserDownloads,
    WindowsArtefacts,
    EntropyRegions,
    RunSummary,
}

impl ParquetCategory {
    fn filename(self) -> &'static str {
        match self {
            ParquetCategory::FilesJpeg => "files_jpeg.parquet",
            ParquetCategory::FilesPng => "files_png.parquet",
            ParquetCategory::FilesGif => "files_gif.parquet",
            ParquetCategory::FilesSqlite => "files_sqlite.parquet",
            ParquetCategory::FilesPdf => "files_pdf.parquet",
            ParquetCategory::FilesZip => "files_zip.parquet",
            ParquetCategory::FilesWebp => "files_webp.parquet",
            ParquetCategory::FilesOther => "files_other.parquet",
            ParquetCategory::ArtefactsUrls => "artefacts_urls.parquet",
            ParquetCategory::ArtefactsEmails => "artefacts_emails.parquet",
            ParquetCategory::ArtefactsPhones => "artefacts_phones.parquet",
            ParquetCategory::ArtefactsBitlockerRecoveryPasswords => {
                "artefacts_bitlocker_recovery_passwords.parquet"
            }
            ParquetCategory::BrowserHistory => "browser_history.parquet",
            ParquetCategory::BrowserCookies => "browser_cookies.parquet",
            ParquetCategory::BrowserDownloads => "browser_downloads.parquet",
            ParquetCategory::WindowsArtefacts => "windows_artefacts.parquet",
            ParquetCategory::EntropyRegions => "entropy_regions.parquet",
            ParquetCategory::RunSummary => "run_summary.parquet",
        }
    }

    fn is_files(self) -> bool {
        matches!(
            self,
            ParquetCategory::FilesJpeg
                | ParquetCategory::FilesPng
                | ParquetCategory::FilesGif
                | ParquetCategory::FilesSqlite
                | ParquetCategory::FilesPdf
                | ParquetCategory::FilesZip
                | ParquetCategory::FilesWebp
                | ParquetCategory::FilesOther
        )
    }
}

#[derive(Debug, Clone)]
struct FileRow {
    handler_id: String,
    file_type: String,
    carved_path: String,
    global_start: i64,
    global_end: i64,
    size: i64,
    md5: Option<String>,
    sha256: Option<String>,
    pattern_id: Option<String>,
    magic_bytes: Option<Vec<u8>>,
    validated: bool,
    truncated: bool,
    error: Option<String>,
    is_duplicate: bool,
    duplicate_of_offset: Option<i64>,
}

#[derive(Debug, Clone)]
struct UrlArtefactRow {
    global_start: i64,
    global_end: i64,
    url: String,
    scheme: String,
    host: String,
    port: Option<i32>,
    path: Option<String>,
    query: Option<String>,
    fragment: Option<String>,
    source_kind: String,
    source_detail: String,
    certainty: f64,
}

#[derive(Debug, Clone)]
struct EmailArtefactRow {
    global_start: i64,
    global_end: i64,
    email: String,
    local_part: String,
    domain: String,
    source_kind: String,
    source_detail: String,
    certainty: f64,
}

#[derive(Debug, Clone)]
struct PhoneArtefactRow {
    global_start: i64,
    global_end: i64,
    phone_raw: String,
    phone_e164: Option<String>,
    country: Option<String>,
    source_kind: String,
    source_detail: String,
    certainty: f64,
}

#[derive(Debug, Clone)]
struct BitlockerRecoveryRow {
    global_start: i64,
    global_end: i64,
    recovery_password: String,
    encoding: String,
    source_kind: String,
    source_detail: String,
    certainty: f64,
}

#[derive(Debug, Clone)]
struct BrowserHistoryRow {
    source_file: String,
    browser: String,
    profile: String,
    url: String,
    title: Option<String>,
    visit_time_utc: Option<i64>,
    visit_source: Option<String>,
    row_id: Option<i64>,
    table_name: Option<String>,
}

#[derive(Debug, Clone)]
struct BrowserCookieRow {
    source_file: String,
    browser: String,
    profile: String,
    host: String,
    name: String,
    value: Option<String>,
    path: Option<String>,
    expires_utc: Option<i64>,
    last_access_utc: Option<i64>,
    creation_utc: Option<i64>,
    is_secure: Option<bool>,
    is_http_only: Option<bool>,
}

#[derive(Debug, Clone)]
struct BrowserDownloadRow {
    source_file: String,
    browser: String,
    profile: String,
    url: Option<String>,
    target_path: Option<String>,
    start_time_utc: Option<i64>,
    end_time_utc: Option<i64>,
    total_bytes: Option<i64>,
    state: Option<String>,
}

#[derive(Debug, Clone)]
struct WindowsArtefactRow {
    artefact_type: String,
    offset: i64,
    size: i64,
    target_path: Option<String>,
    working_dir: Option<String>,
    creation_time_utc: Option<i64>,
    access_time_utc: Option<i64>,
    write_time_utc: Option<i64>,
    file_size: Option<i64>,
    volume_serial: Option<String>,
    local_base_path: Option<String>,
    network_path: Option<String>,
    executable_name: Option<String>,
    prefetch_hash: Option<String>,
    run_count: Option<i64>,
    last_run_times_json: Option<String>,
    volume_paths_json: Option<String>,
    volume_paths_truncated: Option<bool>,
    referenced_files_json: Option<String>,
    version: Option<i32>,
    first_chunk: Option<i64>,
    last_chunk: Option<i64>,
    record_count_estimate: Option<i64>,
    log_name: Option<String>,
    timestamp_utc: Option<i64>,
    hive_name: Option<String>,
    hive_type: Option<String>,
    root_key_name: Option<String>,
}

#[derive(Debug, Clone)]
struct EntropyRegionRow {
    global_start: i64,
    global_end: i64,
    entropy: f64,
    window_size: i64,
}

#[derive(Debug, Clone)]
struct RunSummaryRow {
    bytes_scanned: i64,
    chunks_processed: i64,
    hits_found: i64,
    files_carved: i64,
    files_rejected: i64,
    files_prevalidation_rejected: i64,
    files_capped: i64,
    overlap_skipped: i64,
    string_spans: i64,
    artefacts_extracted: i64,
    duplicates_found: i64,
    duplicates_skipped: i64,
}

enum CategoryBuffer {
    Files(Vec<FileRow>),
    Urls(Vec<UrlArtefactRow>),
    Emails(Vec<EmailArtefactRow>),
    Phones(Vec<PhoneArtefactRow>),
    BitlockerRecoveryPasswords(Vec<BitlockerRecoveryRow>),
    History(Vec<BrowserHistoryRow>),
    Cookies(Vec<BrowserCookieRow>),
    Downloads(Vec<BrowserDownloadRow>),
    Windows(Vec<WindowsArtefactRow>),
    Entropy(Vec<EntropyRegionRow>),
    Summary(Vec<RunSummaryRow>),
}

struct CategoryWriter {
    schema: SchemaRef,
    writer: ArrowWriter<File>,
    buffer: CategoryBuffer,
    row_group_size: usize,
    context: Arc<ParquetContext>,
    finished: bool,
}

impl CategoryWriter {
    fn new(
        path: PathBuf,
        category: ParquetCategory,
        row_group_size: usize,
        context: Arc<ParquetContext>,
    ) -> Result<Self, MetadataError> {
        let schema = schema_for_category(category);
        let props = WriterProperties::builder()
            .set_max_row_group_size(row_group_size)
            .build();
        let file = File::create(path)?;
        let writer = ArrowWriter::try_new(file, schema.clone(), Some(props))
            .map_err(|err| MetadataError::Other(format!("parquet writer error: {err}")))?;
        let buffer = match category {
            ParquetCategory::ArtefactsUrls => CategoryBuffer::Urls(Vec::new()),
            ParquetCategory::ArtefactsEmails => CategoryBuffer::Emails(Vec::new()),
            ParquetCategory::ArtefactsPhones => CategoryBuffer::Phones(Vec::new()),
            ParquetCategory::ArtefactsBitlockerRecoveryPasswords => {
                CategoryBuffer::BitlockerRecoveryPasswords(Vec::new())
            }
            ParquetCategory::BrowserHistory => CategoryBuffer::History(Vec::new()),
            ParquetCategory::BrowserCookies => CategoryBuffer::Cookies(Vec::new()),
            ParquetCategory::BrowserDownloads => CategoryBuffer::Downloads(Vec::new()),
            ParquetCategory::WindowsArtefacts => CategoryBuffer::Windows(Vec::new()),
            ParquetCategory::EntropyRegions => CategoryBuffer::Entropy(Vec::new()),
            ParquetCategory::RunSummary => CategoryBuffer::Summary(Vec::new()),
            _ => CategoryBuffer::Files(Vec::new()),
        };
        Ok(Self {
            schema,
            writer,
            buffer,
            row_group_size: row_group_size.max(1),
            context,
            finished: false,
        })
    }

    fn append_file(&mut self, row: FileRow) -> Result<(), MetadataError> {
        match &mut self.buffer {
            CategoryBuffer::Files(rows) => {
                rows.push(row);
                if rows.len() >= self.row_group_size {
                    self.flush_buffer()?;
                }
                Ok(())
            }
            _ => Err(MetadataError::Other(
                "file row on non-file category".to_string(),
            )),
        }
    }

    fn append_url(&mut self, row: UrlArtefactRow) -> Result<(), MetadataError> {
        match &mut self.buffer {
            CategoryBuffer::Urls(rows) => {
                rows.push(row);
                if rows.len() >= self.row_group_size {
                    self.flush_buffer()?;
                }
                Ok(())
            }
            _ => Err(MetadataError::Other(
                "url row on non-url category".to_string(),
            )),
        }
    }

    fn append_email(&mut self, row: EmailArtefactRow) -> Result<(), MetadataError> {
        match &mut self.buffer {
            CategoryBuffer::Emails(rows) => {
                rows.push(row);
                if rows.len() >= self.row_group_size {
                    self.flush_buffer()?;
                }
                Ok(())
            }
            _ => Err(MetadataError::Other(
                "email row on non-email category".to_string(),
            )),
        }
    }

    fn append_phone(&mut self, row: PhoneArtefactRow) -> Result<(), MetadataError> {
        match &mut self.buffer {
            CategoryBuffer::Phones(rows) => {
                rows.push(row);
                if rows.len() >= self.row_group_size {
                    self.flush_buffer()?;
                }
                Ok(())
            }
            _ => Err(MetadataError::Other(
                "phone row on non-phone category".to_string(),
            )),
        }
    }

    fn append_bitlocker_recovery(
        &mut self,
        row: BitlockerRecoveryRow,
    ) -> Result<(), MetadataError> {
        match &mut self.buffer {
            CategoryBuffer::BitlockerRecoveryPasswords(rows) => {
                rows.push(row);
                if rows.len() >= self.row_group_size {
                    self.flush_buffer()?;
                }
                Ok(())
            }
            _ => Err(MetadataError::Other(
                "bitlocker recovery row on non-bitlocker category".to_string(),
            )),
        }
    }

    fn append_history(&mut self, row: BrowserHistoryRow) -> Result<(), MetadataError> {
        match &mut self.buffer {
            CategoryBuffer::History(rows) => {
                rows.push(row);
                if rows.len() >= self.row_group_size {
                    self.flush_buffer()?;
                }
                Ok(())
            }
            _ => Err(MetadataError::Other(
                "browser history row on non-history category".to_string(),
            )),
        }
    }

    fn append_cookie(&mut self, row: BrowserCookieRow) -> Result<(), MetadataError> {
        match &mut self.buffer {
            CategoryBuffer::Cookies(rows) => {
                rows.push(row);
                if rows.len() >= self.row_group_size {
                    self.flush_buffer()?;
                }
                Ok(())
            }
            _ => Err(MetadataError::Other(
                "browser cookie row on non-cookie category".to_string(),
            )),
        }
    }

    fn append_download(&mut self, row: BrowserDownloadRow) -> Result<(), MetadataError> {
        match &mut self.buffer {
            CategoryBuffer::Downloads(rows) => {
                rows.push(row);
                if rows.len() >= self.row_group_size {
                    self.flush_buffer()?;
                }
                Ok(())
            }
            _ => Err(MetadataError::Other(
                "browser download row on non-download category".to_string(),
            )),
        }
    }

    fn append_windows(&mut self, row: WindowsArtefactRow) -> Result<(), MetadataError> {
        match &mut self.buffer {
            CategoryBuffer::Windows(rows) => {
                rows.push(row);
                if rows.len() >= self.row_group_size {
                    self.flush_buffer()?;
                }
                Ok(())
            }
            _ => Err(MetadataError::Other(
                "windows artefact row on non-windows category".to_string(),
            )),
        }
    }

    fn append_entropy(&mut self, row: EntropyRegionRow) -> Result<(), MetadataError> {
        match &mut self.buffer {
            CategoryBuffer::Entropy(rows) => {
                rows.push(row);
                if rows.len() >= self.row_group_size {
                    self.flush_buffer()?;
                }
                Ok(())
            }
            _ => Err(MetadataError::Other(
                "entropy row on non-entropy category".to_string(),
            )),
        }
    }

    fn append_summary(&mut self, row: RunSummaryRow) -> Result<(), MetadataError> {
        match &mut self.buffer {
            CategoryBuffer::Summary(rows) => {
                rows.push(row);
                if rows.len() >= self.row_group_size {
                    self.flush_buffer()?;
                }
                Ok(())
            }
            _ => Err(MetadataError::Other(
                "run summary row on non-summary category".to_string(),
            )),
        }
    }

    fn flush_buffer(&mut self) -> Result<(), MetadataError> {
        if self.buffer_len() == 0 {
            return Ok(());
        }
        let batch = match &mut self.buffer {
            CategoryBuffer::Files(rows) => {
                let batch = build_files_batch(&self.context, rows, &self.schema)?;
                rows.clear();
                batch
            }
            CategoryBuffer::Urls(rows) => {
                let batch = build_urls_batch(&self.context, rows, &self.schema)?;
                rows.clear();
                batch
            }
            CategoryBuffer::Emails(rows) => {
                let batch = build_emails_batch(&self.context, rows, &self.schema)?;
                rows.clear();
                batch
            }
            CategoryBuffer::Phones(rows) => {
                let batch = build_phones_batch(&self.context, rows, &self.schema)?;
                rows.clear();
                batch
            }
            CategoryBuffer::BitlockerRecoveryPasswords(rows) => {
                let batch = build_bitlocker_recovery_batch(&self.context, rows, &self.schema)?;
                rows.clear();
                batch
            }
            CategoryBuffer::History(rows) => {
                let batch = build_history_batch(&self.context, rows, &self.schema)?;
                rows.clear();
                batch
            }
            CategoryBuffer::Cookies(rows) => {
                let batch = build_cookies_batch(&self.context, rows, &self.schema)?;
                rows.clear();
                batch
            }
            CategoryBuffer::Downloads(rows) => {
                let batch = build_downloads_batch(&self.context, rows, &self.schema)?;
                rows.clear();
                batch
            }
            CategoryBuffer::Windows(rows) => {
                let batch = build_windows_batch(&self.context, rows, &self.schema)?;
                rows.clear();
                batch
            }
            CategoryBuffer::Entropy(rows) => {
                let batch = build_entropy_batch(&self.context, rows, &self.schema)?;
                rows.clear();
                batch
            }
            CategoryBuffer::Summary(rows) => {
                let batch = build_summary_batch(&self.context, rows, &self.schema)?;
                rows.clear();
                batch
            }
        };
        self.writer
            .write(&batch)
            .map_err(|err| MetadataError::Other(format!("parquet write error: {err}")))?;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), MetadataError> {
        if self.finished {
            return Ok(());
        }
        self.flush_buffer()?;
        self.writer
            .finish()
            .map_err(|err| MetadataError::Other(format!("parquet finish error: {err}")))?;
        self.finished = true;
        Ok(())
    }

    fn buffer_len(&self) -> usize {
        match &self.buffer {
            CategoryBuffer::Files(rows) => rows.len(),
            CategoryBuffer::Urls(rows) => rows.len(),
            CategoryBuffer::Emails(rows) => rows.len(),
            CategoryBuffer::Phones(rows) => rows.len(),
            CategoryBuffer::BitlockerRecoveryPasswords(rows) => rows.len(),
            CategoryBuffer::History(rows) => rows.len(),
            CategoryBuffer::Cookies(rows) => rows.len(),
            CategoryBuffer::Downloads(rows) => rows.len(),
            CategoryBuffer::Windows(rows) => rows.len(),
            CategoryBuffer::Entropy(rows) => rows.len(),
            CategoryBuffer::Summary(rows) => rows.len(),
        }
    }
}

struct ParquetSinkInner {
    context: Arc<ParquetContext>,
    parquet_dir: PathBuf,
    row_group_size: usize,
    files_jpeg: Option<CategoryWriter>,
    files_png: Option<CategoryWriter>,
    files_gif: Option<CategoryWriter>,
    files_sqlite: Option<CategoryWriter>,
    files_pdf: Option<CategoryWriter>,
    files_zip: Option<CategoryWriter>,
    files_webp: Option<CategoryWriter>,
    files_other: Option<CategoryWriter>,
    artefacts_urls: Option<CategoryWriter>,
    artefacts_emails: Option<CategoryWriter>,
    artefacts_phones: Option<CategoryWriter>,
    artefacts_bitlocker_recovery_passwords: Option<CategoryWriter>,
    browser_history: Option<CategoryWriter>,
    browser_cookies: Option<CategoryWriter>,
    browser_downloads: Option<CategoryWriter>,
    windows_artefacts: Option<CategoryWriter>,
    entropy_regions: Option<CategoryWriter>,
    run_summary: Option<CategoryWriter>,
}

impl ParquetSinkInner {
    fn get_or_create_writer(
        &mut self,
        category: ParquetCategory,
    ) -> Result<&mut CategoryWriter, MetadataError> {
        let slot = match category {
            ParquetCategory::FilesJpeg => &mut self.files_jpeg,
            ParquetCategory::FilesPng => &mut self.files_png,
            ParquetCategory::FilesGif => &mut self.files_gif,
            ParquetCategory::FilesSqlite => &mut self.files_sqlite,
            ParquetCategory::FilesPdf => &mut self.files_pdf,
            ParquetCategory::FilesZip => &mut self.files_zip,
            ParquetCategory::FilesWebp => &mut self.files_webp,
            ParquetCategory::FilesOther => &mut self.files_other,
            ParquetCategory::ArtefactsUrls => &mut self.artefacts_urls,
            ParquetCategory::ArtefactsEmails => &mut self.artefacts_emails,
            ParquetCategory::ArtefactsPhones => &mut self.artefacts_phones,
            ParquetCategory::ArtefactsBitlockerRecoveryPasswords => {
                &mut self.artefacts_bitlocker_recovery_passwords
            }
            ParquetCategory::BrowserHistory => &mut self.browser_history,
            ParquetCategory::BrowserCookies => &mut self.browser_cookies,
            ParquetCategory::BrowserDownloads => &mut self.browser_downloads,
            ParquetCategory::WindowsArtefacts => &mut self.windows_artefacts,
            ParquetCategory::EntropyRegions => &mut self.entropy_regions,
            ParquetCategory::RunSummary => &mut self.run_summary,
        };

        if slot.is_none() {
            let path = self.parquet_dir.join(category.filename());
            let writer = CategoryWriter::new(
                path,
                category,
                self.row_group_size,
                Arc::clone(&self.context),
            )?;
            *slot = Some(writer);
        }

        slot.as_mut().ok_or_else(|| {
            MetadataError::Other("parquet writer slot missing after init".to_string())
        })
    }

    fn finish_all(&mut self) -> Result<(), MetadataError> {
        if let Some(writer) = &mut self.files_jpeg {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.files_png {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.files_gif {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.files_sqlite {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.files_pdf {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.files_zip {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.files_webp {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.files_other {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.artefacts_urls {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.artefacts_emails {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.artefacts_phones {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.artefacts_bitlocker_recovery_passwords {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.browser_history {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.browser_cookies {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.browser_downloads {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.windows_artefacts {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.entropy_regions {
            writer.finish()?;
        }
        if let Some(writer) = &mut self.run_summary {
            writer.finish()?;
        }
        Ok(())
    }

    /// Flush all writer buffers without finishing (allows continued writes)
    fn flush_all_buffers(&mut self) -> Result<(), MetadataError> {
        if let Some(writer) = &mut self.files_jpeg {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.files_png {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.files_gif {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.files_sqlite {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.files_pdf {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.files_zip {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.files_webp {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.files_other {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.artefacts_urls {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.artefacts_emails {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.artefacts_phones {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.artefacts_bitlocker_recovery_passwords {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.browser_history {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.browser_cookies {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.browser_downloads {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.windows_artefacts {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.entropy_regions {
            writer.flush_buffer()?;
        }
        if let Some(writer) = &mut self.run_summary {
            writer.flush_buffer()?;
        }
        Ok(())
    }
}

pub struct ParquetSink {
    inner: Mutex<ParquetSinkInner>,
}

impl ParquetSink {
    pub fn new(
        cfg: &Config,
        run_id: &str,
        tool_version: &str,
        config_hash: &str,
        evidence_path: &Path,
        evidence_sha256: &str,
        run_output_dir: &Path,
    ) -> Result<Self, MetadataError> {
        let parquet_dir = run_output_dir.join("parquet");
        std::fs::create_dir_all(&parquet_dir)?;
        let context = Arc::new(ParquetContext {
            run_id: run_id.to_string(),
            tool_version: tool_version.to_string(),
            config_hash: config_hash.to_string(),
            evidence_path: evidence_path.to_string_lossy().to_string(),
            evidence_sha256: evidence_sha256.to_string(),
        });

        Ok(Self {
            inner: Mutex::new(ParquetSinkInner {
                context,
                parquet_dir,
                row_group_size: cfg.parquet_row_group_size.max(1),
                files_jpeg: None,
                files_png: None,
                files_gif: None,
                files_sqlite: None,
                files_pdf: None,
                files_zip: None,
                files_webp: None,
                files_other: None,
                artefacts_urls: None,
                artefacts_emails: None,
                artefacts_phones: None,
                artefacts_bitlocker_recovery_passwords: None,
                browser_history: None,
                browser_cookies: None,
                browser_downloads: None,
                windows_artefacts: None,
                entropy_regions: None,
                run_summary: None,
            }),
        })
    }

    fn lock_inner(&self) -> Result<std::sync::MutexGuard<'_, ParquetSinkInner>, MetadataError> {
        self.inner
            .lock()
            .map_err(|_| MetadataError::Other("parquet sink lock poisoned".to_string()))
    }
}

impl MetadataSink for ParquetSink {
    fn record_file(&self, file: &CarvedFile) -> Result<(), MetadataError> {
        let category = category_for_file_type(&file.file_type);
        let row = FileRow {
            handler_id: handler_id_for_file_type(&file.file_type).to_string(),
            file_type: file.file_type.clone(),
            carved_path: file.path.clone(),
            global_start: to_i64(file.global_start)?,
            global_end: to_i64(file.global_end)?,
            size: to_i64(file.size)?,
            md5: file.md5.clone(),
            sha256: file.sha256.clone(),
            pattern_id: file.pattern_id.clone(),
            magic_bytes: None,
            validated: file.validated,
            truncated: file.truncated,
            error: join_errors(&file.errors),
            is_duplicate: file.is_duplicate,
            duplicate_of_offset: file.duplicate_of_offset.map(|v| v as i64),
        };

        let mut inner = self.lock_inner()?;
        let writer = inner.get_or_create_writer(category)?;
        writer.append_file(row)
    }

    fn record_string(&self, artefact: &StringArtefact) -> Result<(), MetadataError> {
        let mut inner = self.lock_inner()?;
        match artefact.artefact_kind {
            ArtefactKind::Url => {
                let row = map_url_artefact(artefact)?;
                let writer = inner.get_or_create_writer(ParquetCategory::ArtefactsUrls)?;
                writer.append_url(row)
            }
            ArtefactKind::Email => {
                let row = map_email_artefact(artefact)?;
                let writer = inner.get_or_create_writer(ParquetCategory::ArtefactsEmails)?;
                writer.append_email(row)
            }
            ArtefactKind::Phone => {
                let row = map_phone_artefact(artefact)?;
                let writer = inner.get_or_create_writer(ParquetCategory::ArtefactsPhones)?;
                writer.append_phone(row)
            }
            ArtefactKind::BitlockerRecoveryPassword => {
                let row = map_bitlocker_recovery_artefact(artefact)?;
                let writer = inner
                    .get_or_create_writer(ParquetCategory::ArtefactsBitlockerRecoveryPasswords)?;
                writer.append_bitlocker_recovery(row)
            }
            ArtefactKind::GenericString => Ok(()),
        }
    }

    fn record_history(&self, record: &BrowserHistoryRecord) -> Result<(), MetadataError> {
        let row = BrowserHistoryRow {
            source_file: record.source_file.to_string_lossy().to_string(),
            browser: record.browser.clone(),
            profile: record.profile.clone(),
            url: record.url.clone(),
            title: record.title.clone(),
            visit_time_utc: record.visit_time.map(to_micros),
            visit_source: record.visit_source.clone(),
            row_id: None,
            table_name: None,
        };

        let mut inner = self.lock_inner()?;
        let writer = inner.get_or_create_writer(ParquetCategory::BrowserHistory)?;
        writer.append_history(row)
    }

    fn record_cookie(&self, record: &BrowserCookieRecord) -> Result<(), MetadataError> {
        let row = BrowserCookieRow {
            source_file: record.source_file.to_string_lossy().to_string(),
            browser: record.browser.clone(),
            profile: record.profile.clone(),
            host: record.host.clone(),
            name: record.name.clone(),
            value: record.value.clone(),
            path: record.path.clone(),
            expires_utc: record.expires_utc.map(to_micros),
            last_access_utc: record.last_access_utc.map(to_micros),
            creation_utc: record.creation_utc.map(to_micros),
            is_secure: record.is_secure,
            is_http_only: record.is_http_only,
        };

        let mut inner = self.lock_inner()?;
        let writer = inner.get_or_create_writer(ParquetCategory::BrowserCookies)?;
        writer.append_cookie(row)
    }

    fn record_download(&self, record: &BrowserDownloadRecord) -> Result<(), MetadataError> {
        let row = BrowserDownloadRow {
            source_file: record.source_file.to_string_lossy().to_string(),
            browser: record.browser.clone(),
            profile: record.profile.clone(),
            url: record.url.clone(),
            target_path: record.target_path.clone(),
            start_time_utc: record.start_time.map(to_micros),
            end_time_utc: record.end_time.map(to_micros),
            total_bytes: record.total_bytes,
            state: record.state.clone(),
        };

        let mut inner = self.lock_inner()?;
        let writer = inner.get_or_create_writer(ParquetCategory::BrowserDownloads)?;
        writer.append_download(row)
    }

    fn record_windows_artefact(&self, record: &WindowsArtefactRecord) -> Result<(), MetadataError> {
        let flat = flatten_windows_artefact(record)?;
        let row = WindowsArtefactRow {
            artefact_type: flat.artefact_type,
            offset: to_i64(flat.offset)?,
            size: to_i64(flat.size)?,
            target_path: flat.target_path,
            working_dir: flat.working_dir,
            creation_time_utc: flat.creation_time.map(to_micros),
            access_time_utc: flat.access_time.map(to_micros),
            write_time_utc: flat.write_time.map(to_micros),
            file_size: flat.file_size.map(|value| value as i64),
            volume_serial: flat.volume_serial,
            local_base_path: flat.local_base_path,
            network_path: flat.network_path,
            executable_name: flat.executable_name,
            prefetch_hash: flat.prefetch_hash,
            run_count: flat.run_count.map(to_i64).transpose()?,
            last_run_times_json: flat.last_run_times_json,
            volume_paths_json: flat.volume_paths_json,
            volume_paths_truncated: flat.volume_paths_truncated,
            referenced_files_json: flat.referenced_files_json,
            version: flat.version.map(to_i32).transpose()?,
            first_chunk: flat.first_chunk.map(to_i64).transpose()?,
            last_chunk: flat.last_chunk.map(to_i64).transpose()?,
            // Overflow on `record_count_estimate` (e.g. an EVTX chunk
            // header with `last_record_number == u64::MAX`) must NOT drop
            // the entire Windows artefact row. Fall back to NULL so the
            // rest of the artefact metadata still reaches Parquet. See
            // GitHub issue #80.
            record_count_estimate: flat
                .record_count_estimate
                .and_then(|value| i64::try_from(value).ok()),
            log_name: flat.log_name,
            timestamp_utc: flat.timestamp.map(to_micros),
            hive_name: flat.hive_name,
            hive_type: flat.hive_type,
            root_key_name: flat.root_key_name,
        };

        let mut inner = self.lock_inner()?;
        let writer = inner.get_or_create_writer(ParquetCategory::WindowsArtefacts)?;
        writer.append_windows(row)
    }

    fn record_run_summary(&self, summary: &RunSummary) -> Result<(), MetadataError> {
        let row = RunSummaryRow {
            bytes_scanned: to_i64(summary.bytes_scanned)?,
            chunks_processed: to_i64(summary.chunks_processed)?,
            hits_found: to_i64(summary.hits_found)?,
            files_carved: to_i64(summary.files_carved)?,
            files_rejected: to_i64(summary.files_rejected)?,
            files_prevalidation_rejected: to_i64(summary.files_prevalidation_rejected)?,
            files_capped: to_i64(summary.files_capped)?,
            overlap_skipped: to_i64(summary.overlap_skipped)?,
            string_spans: to_i64(summary.string_spans)?,
            artefacts_extracted: to_i64(summary.artefacts_extracted)?,
            duplicates_found: to_i64(summary.duplicates_found)?,
            duplicates_skipped: to_i64(summary.duplicates_skipped)?,
        };
        let mut inner = self.lock_inner()?;
        let writer = inner.get_or_create_writer(ParquetCategory::RunSummary)?;
        writer.append_summary(row)
    }

    fn record_entropy(&self, region: &crate::metadata::EntropyRegion) -> Result<(), MetadataError> {
        let row = EntropyRegionRow {
            global_start: to_i64(region.global_start)?,
            global_end: to_i64(region.global_end)?,
            entropy: region.entropy,
            window_size: to_i64(region.window_size)?,
        };
        let mut inner = self.lock_inner()?;
        let writer = inner.get_or_create_writer(ParquetCategory::EntropyRegions)?;
        writer.append_entropy(row)
    }

    fn flush(&self) -> Result<(), MetadataError> {
        // Flush all buffers to ensure data is written to disk
        // This allows recovery of data if the process is interrupted
        let mut inner = self.lock_inner()?;
        inner.flush_all_buffers()?;
        // Note: We don't call finish_all() here because that would close writers
        // and prevent further writes. Finish is called in Drop.
        Ok(())
    }
}

impl Drop for ParquetSink {
    fn drop(&mut self) {
        // Ensure all writers are properly finished when the sink is dropped
        if let Ok(mut inner) = self.inner.lock() {
            let _ = inner.finish_all();
        }
    }
}

pub fn build_parquet_sink(
    cfg: &Config,
    run_id: &str,
    tool_version: &str,
    config_hash: &str,
    evidence_path: &Path,
    evidence_sha256: &str,
    run_output_dir: &Path,
) -> Result<Box<dyn MetadataSink>, MetadataError> {
    Ok(Box::new(ParquetSink::new(
        cfg,
        run_id,
        tool_version,
        config_hash,
        evidence_path,
        evidence_sha256,
        run_output_dir,
    )?))
}

fn handler_id_for_file_type(file_type: &str) -> &str {
    match file_type {
        "docx" | "xlsx" | "pptx" | "zip" => "zip",
        other => other,
    }
}

fn category_for_file_type(file_type: &str) -> ParquetCategory {
    match file_type {
        "jpeg" | "jpg" => ParquetCategory::FilesJpeg,
        "png" => ParquetCategory::FilesPng,
        "gif" => ParquetCategory::FilesGif,
        "sqlite" | "sqlite_db" | "sqlite_wal" | "sqlite_page" => ParquetCategory::FilesSqlite,
        "pdf" => ParquetCategory::FilesPdf,
        "zip" | "docx" | "xlsx" | "pptx" => ParquetCategory::FilesZip,
        "webp" => ParquetCategory::FilesWebp,
        _ => ParquetCategory::FilesOther,
    }
}

fn schema_for_category(category: ParquetCategory) -> SchemaRef {
    if category.is_files() {
        return Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
            Field::new("evidence_sha256", DataType::Utf8, false),
            Field::new("handler_id", DataType::Utf8, false),
            Field::new("file_type", DataType::Utf8, false),
            Field::new("carved_path", DataType::Utf8, false),
            Field::new("global_start", DataType::Int64, false),
            Field::new("global_end", DataType::Int64, false),
            Field::new("size", DataType::Int64, false),
            Field::new("md5", DataType::Utf8, true),
            Field::new("sha256", DataType::Utf8, true),
            Field::new("pattern_id", DataType::Utf8, true),
            Field::new("magic_bytes", DataType::Binary, true),
            Field::new("validated", DataType::Boolean, false),
            Field::new("truncated", DataType::Boolean, false),
            Field::new("error", DataType::Utf8, true),
            Field::new("is_duplicate", DataType::Boolean, false),
            Field::new("duplicate_of_offset", DataType::Int64, true),
        ]));
    }

    match category {
        ParquetCategory::ArtefactsUrls => Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
            Field::new("evidence_sha256", DataType::Utf8, false),
            Field::new("global_start", DataType::Int64, false),
            Field::new("global_end", DataType::Int64, false),
            Field::new("url", DataType::Utf8, false),
            Field::new("scheme", DataType::Utf8, false),
            Field::new("host", DataType::Utf8, false),
            Field::new("port", DataType::Int32, true),
            Field::new("path", DataType::Utf8, true),
            Field::new("query", DataType::Utf8, true),
            Field::new("fragment", DataType::Utf8, true),
            Field::new("source_kind", DataType::Utf8, false),
            Field::new("source_detail", DataType::Utf8, false),
            Field::new("certainty", DataType::Float64, false),
        ])),
        ParquetCategory::ArtefactsEmails => Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
            Field::new("evidence_sha256", DataType::Utf8, false),
            Field::new("global_start", DataType::Int64, false),
            Field::new("global_end", DataType::Int64, false),
            Field::new("email", DataType::Utf8, false),
            Field::new("local_part", DataType::Utf8, false),
            Field::new("domain", DataType::Utf8, false),
            Field::new("source_kind", DataType::Utf8, false),
            Field::new("source_detail", DataType::Utf8, false),
            Field::new("certainty", DataType::Float64, false),
        ])),
        ParquetCategory::ArtefactsPhones => Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
            Field::new("evidence_sha256", DataType::Utf8, false),
            Field::new("global_start", DataType::Int64, false),
            Field::new("global_end", DataType::Int64, false),
            Field::new("phone_raw", DataType::Utf8, false),
            Field::new("phone_e164", DataType::Utf8, true),
            Field::new("country", DataType::Utf8, true),
            Field::new("source_kind", DataType::Utf8, false),
            Field::new("source_detail", DataType::Utf8, false),
            Field::new("certainty", DataType::Float64, false),
        ])),
        ParquetCategory::ArtefactsBitlockerRecoveryPasswords => Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
            Field::new("evidence_sha256", DataType::Utf8, false),
            Field::new("global_start", DataType::Int64, false),
            Field::new("global_end", DataType::Int64, false),
            Field::new("recovery_password", DataType::Utf8, false),
            Field::new("encoding", DataType::Utf8, false),
            Field::new("source_kind", DataType::Utf8, false),
            Field::new("source_detail", DataType::Utf8, false),
            Field::new("certainty", DataType::Float64, false),
        ])),
        ParquetCategory::BrowserHistory => Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
            Field::new("evidence_sha256", DataType::Utf8, false),
            Field::new("source_file", DataType::Utf8, false),
            Field::new("browser", DataType::Utf8, false),
            Field::new("profile", DataType::Utf8, false),
            Field::new("url", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, true),
            Field::new(
                "visit_time_utc",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
            Field::new("visit_source", DataType::Utf8, true),
            Field::new("row_id", DataType::Int64, true),
            Field::new("table_name", DataType::Utf8, true),
        ])),
        ParquetCategory::BrowserCookies => Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
            Field::new("evidence_sha256", DataType::Utf8, false),
            Field::new("source_file", DataType::Utf8, false),
            Field::new("browser", DataType::Utf8, false),
            Field::new("profile", DataType::Utf8, false),
            Field::new("host", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, true),
            Field::new("path", DataType::Utf8, true),
            Field::new(
                "expires_utc",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
            Field::new(
                "last_access_utc",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
            Field::new(
                "creation_utc",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
            Field::new("is_secure", DataType::Boolean, true),
            Field::new("is_http_only", DataType::Boolean, true),
        ])),
        ParquetCategory::BrowserDownloads => Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
            Field::new("evidence_sha256", DataType::Utf8, false),
            Field::new("source_file", DataType::Utf8, false),
            Field::new("browser", DataType::Utf8, false),
            Field::new("profile", DataType::Utf8, false),
            Field::new("url", DataType::Utf8, true),
            Field::new("target_path", DataType::Utf8, true),
            Field::new(
                "start_time_utc",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
            Field::new(
                "end_time_utc",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
            Field::new("total_bytes", DataType::Int64, true),
            Field::new("state", DataType::Utf8, true),
        ])),
        ParquetCategory::WindowsArtefacts => Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
            Field::new("evidence_sha256", DataType::Utf8, false),
            Field::new("artefact_type", DataType::Utf8, false),
            Field::new("offset", DataType::Int64, false),
            Field::new("size", DataType::Int64, false),
            Field::new("target_path", DataType::Utf8, true),
            Field::new("working_dir", DataType::Utf8, true),
            Field::new(
                "creation_time_utc",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
            Field::new(
                "access_time_utc",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
            Field::new(
                "write_time_utc",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
            Field::new("file_size", DataType::Int64, true),
            Field::new("volume_serial", DataType::Utf8, true),
            Field::new("local_base_path", DataType::Utf8, true),
            Field::new("network_path", DataType::Utf8, true),
            Field::new("executable_name", DataType::Utf8, true),
            Field::new("prefetch_hash", DataType::Utf8, true),
            Field::new("run_count", DataType::Int64, true),
            Field::new("last_run_times_json", DataType::Utf8, true),
            Field::new("volume_paths_json", DataType::Utf8, true),
            Field::new("volume_paths_truncated", DataType::Boolean, true),
            Field::new("referenced_files_json", DataType::Utf8, true),
            Field::new("version", DataType::Int32, true),
            Field::new("first_chunk", DataType::Int64, true),
            Field::new("last_chunk", DataType::Int64, true),
            Field::new("record_count_estimate", DataType::Int64, true),
            Field::new("log_name", DataType::Utf8, true),
            Field::new(
                "timestamp_utc",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
            Field::new("hive_name", DataType::Utf8, true),
            Field::new("hive_type", DataType::Utf8, true),
            Field::new("root_key_name", DataType::Utf8, true),
        ])),
        ParquetCategory::EntropyRegions => Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
            Field::new("evidence_sha256", DataType::Utf8, false),
            Field::new("global_start", DataType::Int64, false),
            Field::new("global_end", DataType::Int64, false),
            Field::new("entropy", DataType::Float64, false),
            Field::new("window_size", DataType::Int64, false),
        ])),
        ParquetCategory::RunSummary => Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
            Field::new("evidence_sha256", DataType::Utf8, false),
            Field::new("bytes_scanned", DataType::Int64, false),
            Field::new("chunks_processed", DataType::Int64, false),
            Field::new("hits_found", DataType::Int64, false),
            Field::new("files_carved", DataType::Int64, false),
            Field::new("files_rejected", DataType::Int64, false),
            Field::new("files_prevalidation_rejected", DataType::Int64, false),
            Field::new("files_capped", DataType::Int64, false),
            Field::new("overlap_skipped", DataType::Int64, false),
            Field::new("string_spans", DataType::Int64, false),
            Field::new("artefacts_extracted", DataType::Int64, false),
            Field::new("duplicates_found", DataType::Int64, false),
            Field::new("duplicates_skipped", DataType::Int64, false),
        ])),
        _ => Arc::new(Schema::empty()),
    }
}

fn build_files_batch(
    ctx: &ParquetContext,
    rows: &[FileRow],
    schema: &SchemaRef,
) -> Result<RecordBatch, MetadataError> {
    let mut run_id = StringBuilder::new();
    let mut tool_version = StringBuilder::new();
    let mut config_hash = StringBuilder::new();
    let mut evidence_path = StringBuilder::new();
    let mut evidence_sha256 = StringBuilder::new();
    let mut handler_id = StringBuilder::new();
    let mut file_type = StringBuilder::new();
    let mut carved_path = StringBuilder::new();
    let mut global_start = Int64Builder::new();
    let mut global_end = Int64Builder::new();
    let mut size = Int64Builder::new();
    let mut md5 = StringBuilder::new();
    let mut sha256 = StringBuilder::new();
    let mut pattern_id = StringBuilder::new();
    let mut magic_bytes = BinaryBuilder::new();
    let mut validated = BooleanBuilder::new();
    let mut truncated = BooleanBuilder::new();
    let mut error = StringBuilder::new();
    let mut is_duplicate = BooleanBuilder::new();
    let mut duplicate_of_offset = Int64Builder::new();

    for row in rows {
        run_id.append_value(&ctx.run_id);
        tool_version.append_value(&ctx.tool_version);
        config_hash.append_value(&ctx.config_hash);
        evidence_path.append_value(&ctx.evidence_path);
        evidence_sha256.append_value(&ctx.evidence_sha256);
        handler_id.append_value(&row.handler_id);
        file_type.append_value(&row.file_type);
        carved_path.append_value(&row.carved_path);
        global_start.append_value(row.global_start);
        global_end.append_value(row.global_end);
        size.append_value(row.size);
        md5.append_option(row.md5.as_deref());
        sha256.append_option(row.sha256.as_deref());
        pattern_id.append_option(row.pattern_id.as_deref());
        magic_bytes.append_option(row.magic_bytes.as_deref());
        validated.append_value(row.validated);
        truncated.append_value(row.truncated);
        error.append_option(row.error.as_deref());
        is_duplicate.append_value(row.is_duplicate);
        duplicate_of_offset.append_option(row.duplicate_of_offset);
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(tool_version.finish()),
        Arc::new(config_hash.finish()),
        Arc::new(evidence_path.finish()),
        Arc::new(evidence_sha256.finish()),
        Arc::new(handler_id.finish()),
        Arc::new(file_type.finish()),
        Arc::new(carved_path.finish()),
        Arc::new(global_start.finish()),
        Arc::new(global_end.finish()),
        Arc::new(size.finish()),
        Arc::new(md5.finish()),
        Arc::new(sha256.finish()),
        Arc::new(pattern_id.finish()),
        Arc::new(magic_bytes.finish()),
        Arc::new(validated.finish()),
        Arc::new(truncated.finish()),
        Arc::new(error.finish()),
        Arc::new(is_duplicate.finish()),
        Arc::new(duplicate_of_offset.finish()),
    ];

    RecordBatch::try_new(Arc::clone(schema), arrays)
        .map_err(|err| MetadataError::Other(format!("parquet batch error: {err}")))
}

fn build_urls_batch(
    ctx: &ParquetContext,
    rows: &[UrlArtefactRow],
    schema: &SchemaRef,
) -> Result<RecordBatch, MetadataError> {
    let mut run_id = StringBuilder::new();
    let mut tool_version = StringBuilder::new();
    let mut config_hash = StringBuilder::new();
    let mut evidence_path = StringBuilder::new();
    let mut evidence_sha256 = StringBuilder::new();
    let mut global_start = Int64Builder::new();
    let mut global_end = Int64Builder::new();
    let mut url = StringBuilder::new();
    let mut scheme = StringBuilder::new();
    let mut host = StringBuilder::new();
    let mut port = Int32Builder::new();
    let mut path = StringBuilder::new();
    let mut query = StringBuilder::new();
    let mut fragment = StringBuilder::new();
    let mut source_kind = StringBuilder::new();
    let mut source_detail = StringBuilder::new();
    let mut certainty = arrow_array::builder::Float64Builder::new();

    for row in rows {
        run_id.append_value(&ctx.run_id);
        tool_version.append_value(&ctx.tool_version);
        config_hash.append_value(&ctx.config_hash);
        evidence_path.append_value(&ctx.evidence_path);
        evidence_sha256.append_value(&ctx.evidence_sha256);
        global_start.append_value(row.global_start);
        global_end.append_value(row.global_end);
        url.append_value(&row.url);
        scheme.append_value(&row.scheme);
        host.append_value(&row.host);
        port.append_option(row.port);
        path.append_option(row.path.as_deref());
        query.append_option(row.query.as_deref());
        fragment.append_option(row.fragment.as_deref());
        source_kind.append_value(&row.source_kind);
        source_detail.append_value(&row.source_detail);
        certainty.append_value(row.certainty);
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(tool_version.finish()),
        Arc::new(config_hash.finish()),
        Arc::new(evidence_path.finish()),
        Arc::new(evidence_sha256.finish()),
        Arc::new(global_start.finish()),
        Arc::new(global_end.finish()),
        Arc::new(url.finish()),
        Arc::new(scheme.finish()),
        Arc::new(host.finish()),
        Arc::new(port.finish()),
        Arc::new(path.finish()),
        Arc::new(query.finish()),
        Arc::new(fragment.finish()),
        Arc::new(source_kind.finish()),
        Arc::new(source_detail.finish()),
        Arc::new(certainty.finish()),
    ];

    RecordBatch::try_new(Arc::clone(schema), arrays)
        .map_err(|err| MetadataError::Other(format!("parquet batch error: {err}")))
}

fn build_emails_batch(
    ctx: &ParquetContext,
    rows: &[EmailArtefactRow],
    schema: &SchemaRef,
) -> Result<RecordBatch, MetadataError> {
    let mut run_id = StringBuilder::new();
    let mut tool_version = StringBuilder::new();
    let mut config_hash = StringBuilder::new();
    let mut evidence_path = StringBuilder::new();
    let mut evidence_sha256 = StringBuilder::new();
    let mut global_start = Int64Builder::new();
    let mut global_end = Int64Builder::new();
    let mut email = StringBuilder::new();
    let mut local_part = StringBuilder::new();
    let mut domain = StringBuilder::new();
    let mut source_kind = StringBuilder::new();
    let mut source_detail = StringBuilder::new();
    let mut certainty = arrow_array::builder::Float64Builder::new();

    for row in rows {
        run_id.append_value(&ctx.run_id);
        tool_version.append_value(&ctx.tool_version);
        config_hash.append_value(&ctx.config_hash);
        evidence_path.append_value(&ctx.evidence_path);
        evidence_sha256.append_value(&ctx.evidence_sha256);
        global_start.append_value(row.global_start);
        global_end.append_value(row.global_end);
        email.append_value(&row.email);
        local_part.append_value(&row.local_part);
        domain.append_value(&row.domain);
        source_kind.append_value(&row.source_kind);
        source_detail.append_value(&row.source_detail);
        certainty.append_value(row.certainty);
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(tool_version.finish()),
        Arc::new(config_hash.finish()),
        Arc::new(evidence_path.finish()),
        Arc::new(evidence_sha256.finish()),
        Arc::new(global_start.finish()),
        Arc::new(global_end.finish()),
        Arc::new(email.finish()),
        Arc::new(local_part.finish()),
        Arc::new(domain.finish()),
        Arc::new(source_kind.finish()),
        Arc::new(source_detail.finish()),
        Arc::new(certainty.finish()),
    ];

    RecordBatch::try_new(Arc::clone(schema), arrays)
        .map_err(|err| MetadataError::Other(format!("parquet batch error: {err}")))
}

fn build_phones_batch(
    ctx: &ParquetContext,
    rows: &[PhoneArtefactRow],
    schema: &SchemaRef,
) -> Result<RecordBatch, MetadataError> {
    let mut run_id = StringBuilder::new();
    let mut tool_version = StringBuilder::new();
    let mut config_hash = StringBuilder::new();
    let mut evidence_path = StringBuilder::new();
    let mut evidence_sha256 = StringBuilder::new();
    let mut global_start = Int64Builder::new();
    let mut global_end = Int64Builder::new();
    let mut phone_raw = StringBuilder::new();
    let mut phone_e164 = StringBuilder::new();
    let mut country = StringBuilder::new();
    let mut source_kind = StringBuilder::new();
    let mut source_detail = StringBuilder::new();
    let mut certainty = arrow_array::builder::Float64Builder::new();

    for row in rows {
        run_id.append_value(&ctx.run_id);
        tool_version.append_value(&ctx.tool_version);
        config_hash.append_value(&ctx.config_hash);
        evidence_path.append_value(&ctx.evidence_path);
        evidence_sha256.append_value(&ctx.evidence_sha256);
        global_start.append_value(row.global_start);
        global_end.append_value(row.global_end);
        phone_raw.append_value(&row.phone_raw);
        phone_e164.append_option(row.phone_e164.as_deref());
        country.append_option(row.country.as_deref());
        source_kind.append_value(&row.source_kind);
        source_detail.append_value(&row.source_detail);
        certainty.append_value(row.certainty);
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(tool_version.finish()),
        Arc::new(config_hash.finish()),
        Arc::new(evidence_path.finish()),
        Arc::new(evidence_sha256.finish()),
        Arc::new(global_start.finish()),
        Arc::new(global_end.finish()),
        Arc::new(phone_raw.finish()),
        Arc::new(phone_e164.finish()),
        Arc::new(country.finish()),
        Arc::new(source_kind.finish()),
        Arc::new(source_detail.finish()),
        Arc::new(certainty.finish()),
    ];

    RecordBatch::try_new(Arc::clone(schema), arrays)
        .map_err(|err| MetadataError::Other(format!("parquet batch error: {err}")))
}

fn build_bitlocker_recovery_batch(
    ctx: &ParquetContext,
    rows: &[BitlockerRecoveryRow],
    schema: &SchemaRef,
) -> Result<RecordBatch, MetadataError> {
    let mut run_id = StringBuilder::new();
    let mut tool_version = StringBuilder::new();
    let mut config_hash = StringBuilder::new();
    let mut evidence_path = StringBuilder::new();
    let mut evidence_sha256 = StringBuilder::new();
    let mut global_start = Int64Builder::new();
    let mut global_end = Int64Builder::new();
    let mut recovery_password = StringBuilder::new();
    let mut encoding = StringBuilder::new();
    let mut source_kind = StringBuilder::new();
    let mut source_detail = StringBuilder::new();
    let mut certainty = arrow_array::builder::Float64Builder::new();

    for row in rows {
        run_id.append_value(&ctx.run_id);
        tool_version.append_value(&ctx.tool_version);
        config_hash.append_value(&ctx.config_hash);
        evidence_path.append_value(&ctx.evidence_path);
        evidence_sha256.append_value(&ctx.evidence_sha256);
        global_start.append_value(row.global_start);
        global_end.append_value(row.global_end);
        recovery_password.append_value(&row.recovery_password);
        encoding.append_value(&row.encoding);
        source_kind.append_value(&row.source_kind);
        source_detail.append_value(&row.source_detail);
        certainty.append_value(row.certainty);
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(tool_version.finish()),
        Arc::new(config_hash.finish()),
        Arc::new(evidence_path.finish()),
        Arc::new(evidence_sha256.finish()),
        Arc::new(global_start.finish()),
        Arc::new(global_end.finish()),
        Arc::new(recovery_password.finish()),
        Arc::new(encoding.finish()),
        Arc::new(source_kind.finish()),
        Arc::new(source_detail.finish()),
        Arc::new(certainty.finish()),
    ];

    RecordBatch::try_new(Arc::clone(schema), arrays)
        .map_err(|err| MetadataError::Other(format!("parquet batch error: {err}")))
}

fn build_history_batch(
    ctx: &ParquetContext,
    rows: &[BrowserHistoryRow],
    schema: &SchemaRef,
) -> Result<RecordBatch, MetadataError> {
    let mut run_id = StringBuilder::new();
    let mut tool_version = StringBuilder::new();
    let mut config_hash = StringBuilder::new();
    let mut evidence_path = StringBuilder::new();
    let mut evidence_sha256 = StringBuilder::new();
    let mut source_file = StringBuilder::new();
    let mut browser = StringBuilder::new();
    let mut profile = StringBuilder::new();
    let mut url = StringBuilder::new();
    let mut title = StringBuilder::new();
    let mut visit_time = TimestampMicrosecondBuilder::new();
    let mut visit_source = StringBuilder::new();
    let mut row_id = Int64Builder::new();
    let mut table_name = StringBuilder::new();

    for row in rows {
        run_id.append_value(&ctx.run_id);
        tool_version.append_value(&ctx.tool_version);
        config_hash.append_value(&ctx.config_hash);
        evidence_path.append_value(&ctx.evidence_path);
        evidence_sha256.append_value(&ctx.evidence_sha256);
        source_file.append_value(&row.source_file);
        browser.append_value(&row.browser);
        profile.append_value(&row.profile);
        url.append_value(&row.url);
        title.append_option(row.title.as_deref());
        visit_time.append_option(row.visit_time_utc);
        visit_source.append_option(row.visit_source.as_deref());
        row_id.append_option(row.row_id);
        table_name.append_option(row.table_name.as_deref());
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(tool_version.finish()),
        Arc::new(config_hash.finish()),
        Arc::new(evidence_path.finish()),
        Arc::new(evidence_sha256.finish()),
        Arc::new(source_file.finish()),
        Arc::new(browser.finish()),
        Arc::new(profile.finish()),
        Arc::new(url.finish()),
        Arc::new(title.finish()),
        Arc::new(visit_time.finish()),
        Arc::new(visit_source.finish()),
        Arc::new(row_id.finish()),
        Arc::new(table_name.finish()),
    ];

    RecordBatch::try_new(Arc::clone(schema), arrays)
        .map_err(|err| MetadataError::Other(format!("parquet batch error: {err}")))
}

fn build_cookies_batch(
    ctx: &ParquetContext,
    rows: &[BrowserCookieRow],
    schema: &SchemaRef,
) -> Result<RecordBatch, MetadataError> {
    let mut run_id = StringBuilder::new();
    let mut tool_version = StringBuilder::new();
    let mut config_hash = StringBuilder::new();
    let mut evidence_path = StringBuilder::new();
    let mut evidence_sha256 = StringBuilder::new();
    let mut source_file = StringBuilder::new();
    let mut browser = StringBuilder::new();
    let mut profile = StringBuilder::new();
    let mut host = StringBuilder::new();
    let mut name = StringBuilder::new();
    let mut value = StringBuilder::new();
    let mut path = StringBuilder::new();
    let mut expires = TimestampMicrosecondBuilder::new();
    let mut last_access = TimestampMicrosecondBuilder::new();
    let mut creation = TimestampMicrosecondBuilder::new();
    let mut is_secure = BooleanBuilder::new();
    let mut is_http_only = BooleanBuilder::new();

    for row in rows {
        run_id.append_value(&ctx.run_id);
        tool_version.append_value(&ctx.tool_version);
        config_hash.append_value(&ctx.config_hash);
        evidence_path.append_value(&ctx.evidence_path);
        evidence_sha256.append_value(&ctx.evidence_sha256);
        source_file.append_value(&row.source_file);
        browser.append_value(&row.browser);
        profile.append_value(&row.profile);
        host.append_value(&row.host);
        name.append_value(&row.name);
        value.append_option(row.value.as_deref());
        path.append_option(row.path.as_deref());
        expires.append_option(row.expires_utc);
        last_access.append_option(row.last_access_utc);
        creation.append_option(row.creation_utc);
        is_secure.append_option(row.is_secure);
        is_http_only.append_option(row.is_http_only);
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(tool_version.finish()),
        Arc::new(config_hash.finish()),
        Arc::new(evidence_path.finish()),
        Arc::new(evidence_sha256.finish()),
        Arc::new(source_file.finish()),
        Arc::new(browser.finish()),
        Arc::new(profile.finish()),
        Arc::new(host.finish()),
        Arc::new(name.finish()),
        Arc::new(value.finish()),
        Arc::new(path.finish()),
        Arc::new(expires.finish()),
        Arc::new(last_access.finish()),
        Arc::new(creation.finish()),
        Arc::new(is_secure.finish()),
        Arc::new(is_http_only.finish()),
    ];

    RecordBatch::try_new(Arc::clone(schema), arrays)
        .map_err(|err| MetadataError::Other(format!("parquet batch error: {err}")))
}

fn build_downloads_batch(
    ctx: &ParquetContext,
    rows: &[BrowserDownloadRow],
    schema: &SchemaRef,
) -> Result<RecordBatch, MetadataError> {
    let mut run_id = StringBuilder::new();
    let mut tool_version = StringBuilder::new();
    let mut config_hash = StringBuilder::new();
    let mut evidence_path = StringBuilder::new();
    let mut evidence_sha256 = StringBuilder::new();
    let mut source_file = StringBuilder::new();
    let mut browser = StringBuilder::new();
    let mut profile = StringBuilder::new();
    let mut url = StringBuilder::new();
    let mut target_path = StringBuilder::new();
    let mut start_time = TimestampMicrosecondBuilder::new();
    let mut end_time = TimestampMicrosecondBuilder::new();
    let mut total_bytes = Int64Builder::new();
    let mut state = StringBuilder::new();

    for row in rows {
        run_id.append_value(&ctx.run_id);
        tool_version.append_value(&ctx.tool_version);
        config_hash.append_value(&ctx.config_hash);
        evidence_path.append_value(&ctx.evidence_path);
        evidence_sha256.append_value(&ctx.evidence_sha256);
        source_file.append_value(&row.source_file);
        browser.append_value(&row.browser);
        profile.append_value(&row.profile);
        url.append_option(row.url.as_deref());
        target_path.append_option(row.target_path.as_deref());
        start_time.append_option(row.start_time_utc);
        end_time.append_option(row.end_time_utc);
        total_bytes.append_option(row.total_bytes);
        state.append_option(row.state.as_deref());
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(tool_version.finish()),
        Arc::new(config_hash.finish()),
        Arc::new(evidence_path.finish()),
        Arc::new(evidence_sha256.finish()),
        Arc::new(source_file.finish()),
        Arc::new(browser.finish()),
        Arc::new(profile.finish()),
        Arc::new(url.finish()),
        Arc::new(target_path.finish()),
        Arc::new(start_time.finish()),
        Arc::new(end_time.finish()),
        Arc::new(total_bytes.finish()),
        Arc::new(state.finish()),
    ];

    RecordBatch::try_new(Arc::clone(schema), arrays)
        .map_err(|err| MetadataError::Other(format!("parquet batch error: {err}")))
}

fn build_windows_batch(
    ctx: &ParquetContext,
    rows: &[WindowsArtefactRow],
    schema: &SchemaRef,
) -> Result<RecordBatch, MetadataError> {
    let mut run_id = StringBuilder::new();
    let mut tool_version = StringBuilder::new();
    let mut config_hash = StringBuilder::new();
    let mut evidence_path = StringBuilder::new();
    let mut evidence_sha256 = StringBuilder::new();
    let mut artefact_type = StringBuilder::new();
    let mut offset = Int64Builder::new();
    let mut size = Int64Builder::new();
    let mut target_path = StringBuilder::new();
    let mut working_dir = StringBuilder::new();
    let mut creation_time = TimestampMicrosecondBuilder::new();
    let mut access_time = TimestampMicrosecondBuilder::new();
    let mut write_time = TimestampMicrosecondBuilder::new();
    let mut file_size = Int64Builder::new();
    let mut volume_serial = StringBuilder::new();
    let mut local_base_path = StringBuilder::new();
    let mut network_path = StringBuilder::new();
    let mut executable_name = StringBuilder::new();
    let mut prefetch_hash = StringBuilder::new();
    let mut run_count = Int64Builder::new();
    let mut last_run_times_json = StringBuilder::new();
    let mut volume_paths_json = StringBuilder::new();
    let mut volume_paths_truncated = BooleanBuilder::new();
    let mut referenced_files_json = StringBuilder::new();
    let mut version = Int32Builder::new();
    let mut first_chunk = Int64Builder::new();
    let mut last_chunk = Int64Builder::new();
    let mut record_count_estimate = Int64Builder::new();
    let mut log_name = StringBuilder::new();
    let mut timestamp = TimestampMicrosecondBuilder::new();
    let mut hive_name = StringBuilder::new();
    let mut hive_type = StringBuilder::new();
    let mut root_key_name = StringBuilder::new();

    for row in rows {
        run_id.append_value(&ctx.run_id);
        tool_version.append_value(&ctx.tool_version);
        config_hash.append_value(&ctx.config_hash);
        evidence_path.append_value(&ctx.evidence_path);
        evidence_sha256.append_value(&ctx.evidence_sha256);
        artefact_type.append_value(&row.artefact_type);
        offset.append_value(row.offset);
        size.append_value(row.size);
        target_path.append_option(row.target_path.as_deref());
        working_dir.append_option(row.working_dir.as_deref());
        creation_time.append_option(row.creation_time_utc);
        access_time.append_option(row.access_time_utc);
        write_time.append_option(row.write_time_utc);
        file_size.append_option(row.file_size);
        volume_serial.append_option(row.volume_serial.as_deref());
        local_base_path.append_option(row.local_base_path.as_deref());
        network_path.append_option(row.network_path.as_deref());
        executable_name.append_option(row.executable_name.as_deref());
        prefetch_hash.append_option(row.prefetch_hash.as_deref());
        run_count.append_option(row.run_count);
        last_run_times_json.append_option(row.last_run_times_json.as_deref());
        volume_paths_json.append_option(row.volume_paths_json.as_deref());
        volume_paths_truncated.append_option(row.volume_paths_truncated);
        referenced_files_json.append_option(row.referenced_files_json.as_deref());
        version.append_option(row.version);
        first_chunk.append_option(row.first_chunk);
        last_chunk.append_option(row.last_chunk);
        record_count_estimate.append_option(row.record_count_estimate);
        log_name.append_option(row.log_name.as_deref());
        timestamp.append_option(row.timestamp_utc);
        hive_name.append_option(row.hive_name.as_deref());
        hive_type.append_option(row.hive_type.as_deref());
        root_key_name.append_option(row.root_key_name.as_deref());
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(tool_version.finish()),
        Arc::new(config_hash.finish()),
        Arc::new(evidence_path.finish()),
        Arc::new(evidence_sha256.finish()),
        Arc::new(artefact_type.finish()),
        Arc::new(offset.finish()),
        Arc::new(size.finish()),
        Arc::new(target_path.finish()),
        Arc::new(working_dir.finish()),
        Arc::new(creation_time.finish()),
        Arc::new(access_time.finish()),
        Arc::new(write_time.finish()),
        Arc::new(file_size.finish()),
        Arc::new(volume_serial.finish()),
        Arc::new(local_base_path.finish()),
        Arc::new(network_path.finish()),
        Arc::new(executable_name.finish()),
        Arc::new(prefetch_hash.finish()),
        Arc::new(run_count.finish()),
        Arc::new(last_run_times_json.finish()),
        Arc::new(volume_paths_json.finish()),
        Arc::new(volume_paths_truncated.finish()),
        Arc::new(referenced_files_json.finish()),
        Arc::new(version.finish()),
        Arc::new(first_chunk.finish()),
        Arc::new(last_chunk.finish()),
        Arc::new(record_count_estimate.finish()),
        Arc::new(log_name.finish()),
        Arc::new(timestamp.finish()),
        Arc::new(hive_name.finish()),
        Arc::new(hive_type.finish()),
        Arc::new(root_key_name.finish()),
    ];

    RecordBatch::try_new(Arc::clone(schema), arrays)
        .map_err(|err| MetadataError::Other(format!("parquet batch error: {err}")))
}

fn build_entropy_batch(
    ctx: &ParquetContext,
    rows: &[EntropyRegionRow],
    schema: &SchemaRef,
) -> Result<RecordBatch, MetadataError> {
    let mut run_id = StringBuilder::new();
    let mut tool_version = StringBuilder::new();
    let mut config_hash = StringBuilder::new();
    let mut evidence_path = StringBuilder::new();
    let mut evidence_sha256 = StringBuilder::new();
    let mut global_start = Int64Builder::new();
    let mut global_end = Int64Builder::new();
    let mut entropy = arrow_array::builder::Float64Builder::new();
    let mut window_size = Int64Builder::new();

    for row in rows {
        run_id.append_value(&ctx.run_id);
        tool_version.append_value(&ctx.tool_version);
        config_hash.append_value(&ctx.config_hash);
        evidence_path.append_value(&ctx.evidence_path);
        evidence_sha256.append_value(&ctx.evidence_sha256);
        global_start.append_value(row.global_start);
        global_end.append_value(row.global_end);
        entropy.append_value(row.entropy);
        window_size.append_value(row.window_size);
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(tool_version.finish()),
        Arc::new(config_hash.finish()),
        Arc::new(evidence_path.finish()),
        Arc::new(evidence_sha256.finish()),
        Arc::new(global_start.finish()),
        Arc::new(global_end.finish()),
        Arc::new(entropy.finish()),
        Arc::new(window_size.finish()),
    ];

    RecordBatch::try_new(Arc::clone(schema), arrays)
        .map_err(|err| MetadataError::Other(format!("parquet batch error: {err}")))
}

fn build_summary_batch(
    ctx: &ParquetContext,
    rows: &[RunSummaryRow],
    schema: &SchemaRef,
) -> Result<RecordBatch, MetadataError> {
    let mut run_id = StringBuilder::new();
    let mut tool_version = StringBuilder::new();
    let mut config_hash = StringBuilder::new();
    let mut evidence_path = StringBuilder::new();
    let mut evidence_sha256 = StringBuilder::new();
    let mut bytes_scanned = Int64Builder::new();
    let mut chunks_processed = Int64Builder::new();
    let mut hits_found = Int64Builder::new();
    let mut files_carved = Int64Builder::new();
    let mut files_rejected = Int64Builder::new();
    let mut files_prevalidation_rejected = Int64Builder::new();
    let mut files_capped = Int64Builder::new();
    let mut overlap_skipped = Int64Builder::new();
    let mut string_spans = Int64Builder::new();
    let mut artefacts_extracted = Int64Builder::new();
    let mut duplicates_found = Int64Builder::new();
    let mut duplicates_skipped = Int64Builder::new();

    for row in rows {
        run_id.append_value(&ctx.run_id);
        tool_version.append_value(&ctx.tool_version);
        config_hash.append_value(&ctx.config_hash);
        evidence_path.append_value(&ctx.evidence_path);
        evidence_sha256.append_value(&ctx.evidence_sha256);
        bytes_scanned.append_value(row.bytes_scanned);
        chunks_processed.append_value(row.chunks_processed);
        hits_found.append_value(row.hits_found);
        files_carved.append_value(row.files_carved);
        files_rejected.append_value(row.files_rejected);
        files_prevalidation_rejected.append_value(row.files_prevalidation_rejected);
        files_capped.append_value(row.files_capped);
        overlap_skipped.append_value(row.overlap_skipped);
        string_spans.append_value(row.string_spans);
        artefacts_extracted.append_value(row.artefacts_extracted);
        duplicates_found.append_value(row.duplicates_found);
        duplicates_skipped.append_value(row.duplicates_skipped);
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(tool_version.finish()),
        Arc::new(config_hash.finish()),
        Arc::new(evidence_path.finish()),
        Arc::new(evidence_sha256.finish()),
        Arc::new(bytes_scanned.finish()),
        Arc::new(chunks_processed.finish()),
        Arc::new(hits_found.finish()),
        Arc::new(files_carved.finish()),
        Arc::new(files_rejected.finish()),
        Arc::new(files_prevalidation_rejected.finish()),
        Arc::new(files_capped.finish()),
        Arc::new(overlap_skipped.finish()),
        Arc::new(string_spans.finish()),
        Arc::new(artefacts_extracted.finish()),
        Arc::new(duplicates_found.finish()),
        Arc::new(duplicates_skipped.finish()),
    ];

    RecordBatch::try_new(Arc::clone(schema), arrays)
        .map_err(|err| MetadataError::Other(format!("parquet batch error: {err}")))
}

fn map_url_artefact(artefact: &StringArtefact) -> Result<UrlArtefactRow, MetadataError> {
    let (scheme, host, port, path, query, fragment) = parse_url_parts(&artefact.content);
    Ok(UrlArtefactRow {
        global_start: to_i64(artefact.global_start)?,
        global_end: to_i64(artefact.global_end)?,
        url: artefact.content.clone(),
        scheme,
        host,
        port,
        path,
        query,
        fragment,
        source_kind: "string_span".to_string(),
        source_detail: "strings_artefacts".to_string(),
        certainty: 1.0,
    })
}

fn map_email_artefact(artefact: &StringArtefact) -> Result<EmailArtefactRow, MetadataError> {
    let (local_part, domain) = split_email(&artefact.content);
    Ok(EmailArtefactRow {
        global_start: to_i64(artefact.global_start)?,
        global_end: to_i64(artefact.global_end)?,
        email: artefact.content.clone(),
        local_part,
        domain,
        source_kind: "string_span".to_string(),
        source_detail: "strings_artefacts".to_string(),
        certainty: 1.0,
    })
}

fn map_phone_artefact(artefact: &StringArtefact) -> Result<PhoneArtefactRow, MetadataError> {
    Ok(PhoneArtefactRow {
        global_start: to_i64(artefact.global_start)?,
        global_end: to_i64(artefact.global_end)?,
        phone_raw: artefact.content.clone(),
        phone_e164: None,
        country: None,
        source_kind: "string_span".to_string(),
        source_detail: "strings_artefacts".to_string(),
        certainty: 1.0,
    })
}

fn map_bitlocker_recovery_artefact(
    artefact: &StringArtefact,
) -> Result<BitlockerRecoveryRow, MetadataError> {
    Ok(BitlockerRecoveryRow {
        global_start: to_i64(artefact.global_start)?,
        global_end: to_i64(artefact.global_end)?,
        recovery_password: artefact.content.clone(),
        encoding: artefact.encoding.clone(),
        source_kind: "string_span".to_string(),
        source_detail: "strings_artefacts".to_string(),
        certainty: 1.0,
    })
}

fn parse_url_parts(
    url: &str,
) -> (
    String,
    String,
    Option<i32>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let mut scheme = String::new();
    let mut rest = url;
    if let Some(stripped) = url.strip_prefix("http://") {
        scheme = "http".to_string();
        rest = stripped;
    } else if let Some(stripped) = url.strip_prefix("https://") {
        scheme = "https".to_string();
        rest = stripped;
    } else if let Some(stripped) = url.strip_prefix("ftp://") {
        scheme = "ftp".to_string();
        rest = stripped;
    } else if let Some(stripped) = url.strip_prefix("ftps://") {
        scheme = "ftps".to_string();
        rest = stripped;
    } else if url.starts_with("www.") {
        scheme = "http".to_string();
        rest = url;
    } else if let Some(stripped) = url.strip_prefix("//") {
        scheme = "http".to_string();
        rest = stripped;
    }

    let mut fragment = None;
    let mut query = None;
    let mut path = None;

    let mut base = rest;
    if let Some(pos) = base.find('#') {
        fragment = Some(base[pos + 1..].to_string());
        base = &base[..pos];
    }
    if let Some(pos) = base.find('?') {
        query = Some(base[pos + 1..].to_string());
        base = &base[..pos];
    }
    if let Some(pos) = base.find('/') {
        path = Some(base[pos..].to_string());
        base = &base[..pos];
    }

    let mut host = base.to_string();
    let mut port = None;
    if let Some(pos) = base.rfind(':')
        && !base[pos + 1..].is_empty()
        && base[pos + 1..].chars().all(|c| c.is_ascii_digit())
        && let Ok(parsed) = base[pos + 1..].parse::<i32>()
    {
        port = Some(parsed);
        host = base[..pos].to_string();
    }

    (scheme, host, port, path, query, fragment)
}

fn split_email(value: &str) -> (String, String) {
    if let Some(pos) = value.find('@') {
        let local = &value[..pos];
        let domain = &value[pos + 1..];
        (local.to_string(), domain.to_string())
    } else {
        (String::new(), String::new())
    }
}

fn join_errors(errors: &[String]) -> Option<String> {
    if errors.is_empty() {
        None
    } else {
        Some(errors.join("; "))
    }
}

fn to_i64(value: u64) -> Result<i64, MetadataError> {
    i64::try_from(value).map_err(|_| MetadataError::Other("value exceeds i64 range".to_string()))
}

fn to_i32(value: u32) -> Result<i32, MetadataError> {
    i32::try_from(value).map_err(|_| MetadataError::Other("value exceeds i32 range".to_string()))
}

fn to_micros(value: chrono::NaiveDateTime) -> i64 {
    let utc = value.and_utc();
    let seconds = utc.timestamp();
    let micros = i64::from(utc.timestamp_subsec_micros());
    seconds.saturating_mul(1_000_000).saturating_add(micros)
}
