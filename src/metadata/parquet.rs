use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arrow_array::builder::{
    BinaryBuilder,
    BooleanBuilder,
    Int32Builder,
    Int64Builder,
    StringBuilder,
    TimestampMicrosecondBuilder,
};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use crate::carve::CarvedFile;
use crate::config::Config;
use crate::metadata::{MetadataError, MetadataSink};
use crate::parsers::browser::BrowserHistoryRecord;
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
    BrowserHistory,
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
            ParquetCategory::BrowserHistory => "browser_history.parquet",
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

enum CategoryBuffer {
    Files(Vec<FileRow>),
    Urls(Vec<UrlArtefactRow>),
    Emails(Vec<EmailArtefactRow>),
    Phones(Vec<PhoneArtefactRow>),
    History(Vec<BrowserHistoryRow>),
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
            ParquetCategory::BrowserHistory => CategoryBuffer::History(Vec::new()),
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
            _ => Err(MetadataError::Other("file row on non-file category".to_string())),
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
            _ => Err(MetadataError::Other("url row on non-url category".to_string())),
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
            _ => Err(MetadataError::Other("email row on non-email category".to_string())),
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
            _ => Err(MetadataError::Other("phone row on non-phone category".to_string())),
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
            CategoryBuffer::History(rows) => {
                let batch = build_history_batch(&self.context, rows, &self.schema)?;
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
            CategoryBuffer::History(rows) => rows.len(),
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
    browser_history: Option<CategoryWriter>,
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
            ParquetCategory::BrowserHistory => &mut self.browser_history,
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

        Ok(slot.as_mut().unwrap())
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
        if let Some(writer) = &mut self.browser_history {
            writer.finish()?;
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
                browser_history: None,
            }),
        })
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
        };

        let mut inner = self.inner.lock().unwrap();
        let writer = inner.get_or_create_writer(category)?;
        writer.append_file(row)
    }

    fn record_string(&self, artefact: &StringArtefact) -> Result<(), MetadataError> {
        let mut inner = self.inner.lock().unwrap();
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

        let mut inner = self.inner.lock().unwrap();
        let writer = inner.get_or_create_writer(ParquetCategory::BrowserHistory)?;
        writer.append_history(row)
    }

    fn flush(&self) -> Result<(), MetadataError> {
        let mut inner = self.inner.lock().unwrap();
        inner.finish_all()
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
        "sqlite" | "sqlite_db" => ParquetCategory::FilesSqlite,
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
        ]));
    }

    match category {
        ParquetCategory::ArtefactsUrls => Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
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
            Field::new("global_start", DataType::Int64, false),
            Field::new("global_end", DataType::Int64, false),
            Field::new("phone_raw", DataType::Utf8, false),
            Field::new("phone_e164", DataType::Utf8, true),
            Field::new("country", DataType::Utf8, true),
            Field::new("source_kind", DataType::Utf8, false),
            Field::new("source_detail", DataType::Utf8, false),
            Field::new("certainty", DataType::Float64, false),
        ])),
        ParquetCategory::BrowserHistory => Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("tool_version", DataType::Utf8, false),
            Field::new("config_hash", DataType::Utf8, false),
            Field::new("evidence_path", DataType::Utf8, false),
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

fn build_history_batch(
    ctx: &ParquetContext,
    rows: &[BrowserHistoryRow],
    schema: &SchemaRef,
) -> Result<RecordBatch, MetadataError> {
    let mut run_id = StringBuilder::new();
    let mut tool_version = StringBuilder::new();
    let mut config_hash = StringBuilder::new();
    let mut evidence_path = StringBuilder::new();
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
    } else if url.starts_with("www.") {
        scheme = "http".to_string();
        rest = url;
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
    if let Some(pos) = base.rfind(':') {
        let candidate = &base[pos + 1..];
        if !candidate.is_empty() && candidate.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(parsed) = candidate.parse::<i32>() {
                port = Some(parsed);
                host = base[..pos].to_string();
            }
        }
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
    i64::try_from(value)
        .map_err(|_| MetadataError::Other("value exceeds i64 range".to_string()))
}

fn to_micros(value: chrono::NaiveDateTime) -> i64 {
    let utc = value.and_utc();
    let seconds = utc.timestamp();
    let micros = i64::from(utc.timestamp_subsec_micros());
    seconds.saturating_mul(1_000_000).saturating_add(micros)
}
