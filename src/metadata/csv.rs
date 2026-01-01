use std::fs::File;
use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;

use crate::carve::CarvedFile;
use crate::metadata::{EntropyRegion, MetadataError, MetadataSink, RunSummary};
use crate::parsers::browser::{BrowserCookieRecord, BrowserDownloadRecord};
use crate::strings::artifacts::{ArtefactKind, StringArtefact};

pub struct CsvSink {
    tool_version: String,
    config_hash: String,
    evidence_path: String,
    evidence_sha256: String,
    files_writer: Mutex<csv::Writer<File>>,
    strings_writer: Mutex<csv::Writer<File>>,
    history_writer: Mutex<csv::Writer<File>>,
    cookies_writer: Mutex<csv::Writer<File>>,
    downloads_writer: Mutex<csv::Writer<File>>,
    run_writer: Mutex<csv::Writer<File>>,
    entropy_writer: Mutex<csv::Writer<File>>,
}

#[derive(Serialize)]
struct CarvedFileCsv<'a> {
    run_id: &'a str,
    file_type: &'a str,
    path: &'a str,
    extension: &'a str,
    global_start: u64,
    global_end: u64,
    size: u64,
    md5: Option<&'a str>,
    sha256: Option<&'a str>,
    validated: bool,
    truncated: bool,
    errors: String,
    pattern_id: Option<&'a str>,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

#[derive(Serialize)]
struct StringArtefactCsv<'a> {
    run_id: &'a str,
    artefact_kind: &'a str,
    content: &'a str,
    encoding: &'a str,
    global_start: u64,
    global_end: u64,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

#[derive(Serialize)]
struct BrowserHistoryCsv<'a> {
    run_id: &'a str,
    browser: &'a str,
    profile: &'a str,
    url: &'a str,
    title: Option<&'a str>,
    visit_time: Option<String>,
    visit_source: Option<&'a str>,
    source_file: String,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

#[derive(Serialize)]
struct BrowserCookieCsv<'a> {
    run_id: &'a str,
    browser: &'a str,
    profile: &'a str,
    host: &'a str,
    name: &'a str,
    value: Option<&'a str>,
    path: Option<&'a str>,
    expires_utc: Option<String>,
    last_access_utc: Option<String>,
    creation_utc: Option<String>,
    is_secure: Option<bool>,
    is_http_only: Option<bool>,
    source_file: String,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

#[derive(Serialize)]
struct BrowserDownloadCsv<'a> {
    run_id: &'a str,
    browser: &'a str,
    profile: &'a str,
    url: Option<&'a str>,
    target_path: Option<&'a str>,
    start_time: Option<String>,
    end_time: Option<String>,
    total_bytes: Option<i64>,
    state: Option<&'a str>,
    source_file: String,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

#[derive(Serialize)]
struct RunSummaryCsv<'a> {
    run_id: &'a str,
    bytes_scanned: u64,
    chunks_processed: u64,
    hits_found: u64,
    files_carved: u64,
    string_spans: u64,
    artefacts_extracted: u64,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

#[derive(Serialize)]
struct EntropyRegionCsv<'a> {
    run_id: &'a str,
    global_start: u64,
    global_end: u64,
    entropy: f64,
    window_size: u64,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

impl CsvSink {
    pub fn new(
        _run_id: &str,
        tool_version: &str,
        config_hash: &str,
        evidence_path: &Path,
        evidence_sha256: &str,
        run_output_dir: &Path,
    ) -> Result<Self, MetadataError> {
        let meta_dir = run_output_dir.join("metadata");
        std::fs::create_dir_all(&meta_dir)?;

        let files_file = File::create(meta_dir.join("carved_files.csv"))?;
        let strings_file = File::create(meta_dir.join("string_artefacts.csv"))?;
        let history_file = File::create(meta_dir.join("browser_history.csv"))?;
        let cookies_file = File::create(meta_dir.join("browser_cookies.csv"))?;
        let downloads_file = File::create(meta_dir.join("browser_downloads.csv"))?;
        let run_file = File::create(meta_dir.join("run_summary.csv"))?;
        let entropy_file = File::create(meta_dir.join("entropy_regions.csv"))?;

        let mut files_writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(files_file);
        let mut strings_writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(strings_file);
        let mut history_writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(history_file);
        let mut cookies_writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(cookies_file);
        let mut downloads_writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(downloads_file);
        let mut run_writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(run_file);
        let mut entropy_writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(entropy_file);

        files_writer.write_record(&[
            "run_id",
            "file_type",
            "path",
            "extension",
            "global_start",
            "global_end",
            "size",
            "md5",
            "sha256",
            "validated",
            "truncated",
            "errors",
            "pattern_id",
            "tool_version",
            "config_hash",
            "evidence_path",
            "evidence_sha256",
        ])?;

        strings_writer.write_record(&[
            "run_id",
            "artefact_kind",
            "content",
            "encoding",
            "global_start",
            "global_end",
            "tool_version",
            "config_hash",
            "evidence_path",
            "evidence_sha256",
        ])?;

        history_writer.write_record(&[
            "run_id",
            "browser",
            "profile",
            "url",
            "title",
            "visit_time",
            "visit_source",
            "source_file",
            "tool_version",
            "config_hash",
            "evidence_path",
            "evidence_sha256",
        ])?;

        cookies_writer.write_record(&[
            "run_id",
            "browser",
            "profile",
            "host",
            "name",
            "value",
            "path",
            "expires_utc",
            "last_access_utc",
            "creation_utc",
            "is_secure",
            "is_http_only",
            "source_file",
            "tool_version",
            "config_hash",
            "evidence_path",
            "evidence_sha256",
        ])?;

        downloads_writer.write_record(&[
            "run_id",
            "browser",
            "profile",
            "url",
            "target_path",
            "start_time",
            "end_time",
            "total_bytes",
            "state",
            "source_file",
            "tool_version",
            "config_hash",
            "evidence_path",
            "evidence_sha256",
        ])?;

        run_writer.write_record(&[
            "run_id",
            "bytes_scanned",
            "chunks_processed",
            "hits_found",
            "files_carved",
            "string_spans",
            "artefacts_extracted",
            "tool_version",
            "config_hash",
            "evidence_path",
            "evidence_sha256",
        ])?;

        entropy_writer.write_record(&[
            "run_id",
            "global_start",
            "global_end",
            "entropy",
            "window_size",
            "tool_version",
            "config_hash",
            "evidence_path",
            "evidence_sha256",
        ])?;

        Ok(Self {
            tool_version: tool_version.to_string(),
            config_hash: config_hash.to_string(),
            evidence_path: evidence_path.to_string_lossy().to_string(),
            evidence_sha256: evidence_sha256.to_string(),
            files_writer: Mutex::new(files_writer),
            strings_writer: Mutex::new(strings_writer),
            history_writer: Mutex::new(history_writer),
            cookies_writer: Mutex::new(cookies_writer),
            downloads_writer: Mutex::new(downloads_writer),
            run_writer: Mutex::new(run_writer),
            entropy_writer: Mutex::new(entropy_writer),
        })
    }
}

impl MetadataSink for CsvSink {
    fn record_file(&self, file: &CarvedFile) -> Result<(), MetadataError> {
        let record = CarvedFileCsv {
            run_id: &file.run_id,
            file_type: &file.file_type,
            path: &file.path,
            extension: &file.extension,
            global_start: file.global_start,
            global_end: file.global_end,
            size: file.size,
            md5: file.md5.as_deref(),
            sha256: file.sha256.as_deref(),
            validated: file.validated,
            truncated: file.truncated,
            errors: file.errors.join("; "),
            pattern_id: file.pattern_id.as_deref(),
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self
            .files_writer
            .lock()
            .map_err(|_| MetadataError::Other("files writer lock poisoned".into()))?;
        guard.serialize(record)?;
        Ok(())
    }

    fn record_string(&self, artefact: &StringArtefact) -> Result<(), MetadataError> {
        let record = StringArtefactCsv {
            run_id: &artefact.run_id,
            artefact_kind: artefact_kind_label(&artefact.artefact_kind),
            content: &artefact.content,
            encoding: &artefact.encoding,
            global_start: artefact.global_start,
            global_end: artefact.global_end,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self
            .strings_writer
            .lock()
            .map_err(|_| MetadataError::Other("strings writer lock poisoned".into()))?;
        guard.serialize(record)?;
        Ok(())
    }

    fn record_history(
        &self,
        record: &crate::parsers::browser::BrowserHistoryRecord,
    ) -> Result<(), MetadataError> {
        let record = BrowserHistoryCsv {
            run_id: &record.run_id,
            browser: &record.browser,
            profile: &record.profile,
            url: &record.url,
            title: record.title.as_deref(),
            visit_time: record.visit_time.map(|t| t.to_string()),
            visit_source: record.visit_source.as_deref(),
            source_file: record.source_file.to_string_lossy().to_string(),
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self
            .history_writer
            .lock()
            .map_err(|_| MetadataError::Other("history writer lock poisoned".into()))?;
        guard.serialize(record)?;
        Ok(())
    }

    fn record_cookie(&self, record: &BrowserCookieRecord) -> Result<(), MetadataError> {
        let record = BrowserCookieCsv {
            run_id: &record.run_id,
            browser: &record.browser,
            profile: &record.profile,
            host: &record.host,
            name: &record.name,
            value: record.value.as_deref(),
            path: record.path.as_deref(),
            expires_utc: record.expires_utc.map(|dt| dt.to_string()),
            last_access_utc: record.last_access_utc.map(|dt| dt.to_string()),
            creation_utc: record.creation_utc.map(|dt| dt.to_string()),
            is_secure: record.is_secure,
            is_http_only: record.is_http_only,
            source_file: record.source_file.to_string_lossy().to_string(),
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self
            .cookies_writer
            .lock()
            .map_err(|_| MetadataError::Other("cookies writer lock poisoned".into()))?;
        guard.serialize(record)?;
        Ok(())
    }

    fn record_download(&self, record: &BrowserDownloadRecord) -> Result<(), MetadataError> {
        let record = BrowserDownloadCsv {
            run_id: &record.run_id,
            browser: &record.browser,
            profile: &record.profile,
            url: record.url.as_deref(),
            target_path: record.target_path.as_deref(),
            start_time: record.start_time.map(|dt| dt.to_string()),
            end_time: record.end_time.map(|dt| dt.to_string()),
            total_bytes: record.total_bytes,
            state: record.state.as_deref(),
            source_file: record.source_file.to_string_lossy().to_string(),
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self
            .downloads_writer
            .lock()
            .map_err(|_| MetadataError::Other("downloads writer lock poisoned".into()))?;
        guard.serialize(record)?;
        Ok(())
    }

    fn record_run_summary(&self, summary: &RunSummary) -> Result<(), MetadataError> {
        let record = RunSummaryCsv {
            run_id: &summary.run_id,
            bytes_scanned: summary.bytes_scanned,
            chunks_processed: summary.chunks_processed,
            hits_found: summary.hits_found,
            files_carved: summary.files_carved,
            string_spans: summary.string_spans,
            artefacts_extracted: summary.artefacts_extracted,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self
            .run_writer
            .lock()
            .map_err(|_| MetadataError::Other("run writer lock poisoned".into()))?;
        guard.serialize(record)?;
        Ok(())
    }

    fn record_entropy(&self, region: &EntropyRegion) -> Result<(), MetadataError> {
        let record = EntropyRegionCsv {
            run_id: &region.run_id,
            global_start: region.global_start,
            global_end: region.global_end,
            entropy: region.entropy,
            window_size: region.window_size,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self
            .entropy_writer
            .lock()
            .map_err(|_| MetadataError::Other("entropy writer lock poisoned".into()))?;
        guard.serialize(record)?;
        Ok(())
    }

    fn flush(&self) -> Result<(), MetadataError> {
        let mut files = self
            .files_writer
            .lock()
            .map_err(|_| MetadataError::Other("files writer lock poisoned".into()))?;
        let mut strings = self
            .strings_writer
            .lock()
            .map_err(|_| MetadataError::Other("strings writer lock poisoned".into()))?;
        let mut history = self
            .history_writer
            .lock()
            .map_err(|_| MetadataError::Other("history writer lock poisoned".into()))?;
        let mut cookies = self
            .cookies_writer
            .lock()
            .map_err(|_| MetadataError::Other("cookies writer lock poisoned".into()))?;
        let mut downloads = self
            .downloads_writer
            .lock()
            .map_err(|_| MetadataError::Other("downloads writer lock poisoned".into()))?;
        let mut run = self
            .run_writer
            .lock()
            .map_err(|_| MetadataError::Other("run writer lock poisoned".into()))?;
        let mut entropy = self
            .entropy_writer
            .lock()
            .map_err(|_| MetadataError::Other("entropy writer lock poisoned".into()))?;
        files.flush()?;
        strings.flush()?;
        history.flush()?;
        cookies.flush()?;
        downloads.flush()?;
        run.flush()?;
        entropy.flush()?;
        Ok(())
    }
}

fn artefact_kind_label(kind: &ArtefactKind) -> &'static str {
    match kind {
        ArtefactKind::Url => "url",
        ArtefactKind::Email => "email",
        ArtefactKind::Phone => "phone",
        ArtefactKind::GenericString => "string",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::RunSummary;
    use tempfile::tempdir;

    #[test]
    fn writes_csv_files() {
        let dir = tempdir().expect("tempdir");
        let sink = CsvSink::new(
            "run1",
            "0.1.0",
            "hash",
            Path::new("/evidence.dd"),
            "",
            dir.path(),
        )
        .expect("csv sink");

        let file = CarvedFile {
            run_id: "run1".to_string(),
            file_type: "jpeg".to_string(),
            path: "jpeg/file.jpg".to_string(),
            extension: "jpg".to_string(),
            global_start: 0,
            global_end: 10,
            size: 11,
            md5: None,
            sha256: None,
            validated: true,
            truncated: false,
            errors: Vec::new(),
            pattern_id: Some("jpeg_soi".to_string()),
        };
        sink.record_file(&file).expect("record file");

        let artefact = StringArtefact {
            run_id: "run1".to_string(),
            artefact_kind: ArtefactKind::Url,
            content: "https://example.com".to_string(),
            encoding: "ascii".to_string(),
            global_start: 100,
            global_end: 120,
        };
        sink.record_string(&artefact).expect("record string");

        let history = crate::parsers::browser::BrowserHistoryRecord {
            run_id: "run1".to_string(),
            browser: "chrome".to_string(),
            profile: "Default".to_string(),
            url: "https://example.com".to_string(),
            title: Some("Example".to_string()),
            visit_time: None,
            visit_source: None,
            source_file: "sqlite/history.sqlite".into(),
        };
        sink.record_history(&history).expect("record history");

        let cookie = crate::parsers::browser::BrowserCookieRecord {
            run_id: "run1".to_string(),
            browser: "chrome".to_string(),
            profile: "Default".to_string(),
            host: "example.com".to_string(),
            name: "sid".to_string(),
            value: Some("abc123".to_string()),
            path: Some("/".to_string()),
            expires_utc: None,
            last_access_utc: None,
            creation_utc: None,
            is_secure: Some(true),
            is_http_only: Some(true),
            source_file: "sqlite/Cookies".into(),
        };
        sink.record_cookie(&cookie).expect("record cookie");

        let download = crate::parsers::browser::BrowserDownloadRecord {
            run_id: "run1".to_string(),
            browser: "chrome".to_string(),
            profile: "Default".to_string(),
            url: Some("https://example.com/file.zip".to_string()),
            target_path: Some("/tmp/file.zip".to_string()),
            start_time: None,
            end_time: None,
            total_bytes: Some(123),
            state: Some("1".to_string()),
            source_file: "sqlite/History".into(),
        };
        sink.record_download(&download).expect("record download");
        let summary = RunSummary {
            run_id: "run1".to_string(),
            bytes_scanned: 10,
            chunks_processed: 1,
            hits_found: 2,
            files_carved: 1,
            string_spans: 3,
            artefacts_extracted: 4,
        };
        sink.record_run_summary(&summary).expect("record summary");
        let region = EntropyRegion {
            run_id: "run1".to_string(),
            global_start: 0,
            global_end: 15,
            entropy: 7.9,
            window_size: 16,
        };
        sink.record_entropy(&region).expect("record entropy");

        sink.flush().expect("flush");

        assert!(dir
            .path()
            .join("metadata")
            .join("carved_files.csv")
            .exists());
        assert!(dir
            .path()
            .join("metadata")
            .join("string_artefacts.csv")
            .exists());
        assert!(dir
            .path()
            .join("metadata")
            .join("browser_history.csv")
            .exists());
        assert!(dir
            .path()
            .join("metadata")
            .join("browser_cookies.csv")
            .exists());
        assert!(dir
            .path()
            .join("metadata")
            .join("browser_downloads.csv")
            .exists());
        assert!(dir.path().join("metadata").join("run_summary.csv").exists());
        assert!(dir
            .path()
            .join("metadata")
            .join("entropy_regions.csv")
            .exists());
    }
}
