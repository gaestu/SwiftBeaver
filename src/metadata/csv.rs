use std::fs::File;
use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;

use crate::carve::CarvedFile;
use crate::metadata::{EntropyRegion, MetadataError, MetadataSink, RunSummary};
use crate::strings::artifacts::{ArtefactKind, StringArtefact};

pub struct CsvSink {
    tool_version: String,
    config_hash: String,
    evidence_path: String,
    evidence_sha256: String,
    files_writer: Mutex<csv::Writer<File>>,
    strings_writer: Mutex<csv::Writer<File>>,
    history_writer: Mutex<csv::Writer<File>>,
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
        let run_file = File::create(meta_dir.join("run_summary.csv"))?;
        let entropy_file = File::create(meta_dir.join("entropy_regions.csv"))?;

        let mut files_writer = csv::WriterBuilder::new().has_headers(false).from_writer(files_file);
        let mut strings_writer = csv::WriterBuilder::new().has_headers(false).from_writer(strings_file);
        let mut history_writer = csv::WriterBuilder::new().has_headers(false).from_writer(history_file);
        let mut run_writer = csv::WriterBuilder::new().has_headers(false).from_writer(run_file);
        let mut entropy_writer = csv::WriterBuilder::new().has_headers(false).from_writer(entropy_file);

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
        let mut guard = self.files_writer.lock().unwrap();
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
        let mut guard = self.strings_writer.lock().unwrap();
        guard.serialize(record)?;
        Ok(())
    }

    fn record_history(&self, record: &crate::parsers::browser::BrowserHistoryRecord) -> Result<(), MetadataError> {
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
        let mut guard = self.history_writer.lock().unwrap();
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
        let mut guard = self.run_writer.lock().unwrap();
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
        let mut guard = self.entropy_writer.lock().unwrap();
        guard.serialize(record)?;
        Ok(())
    }

    fn flush(&self) -> Result<(), MetadataError> {
        let mut files = self.files_writer.lock().unwrap();
        let mut strings = self.strings_writer.lock().unwrap();
        let mut history = self.history_writer.lock().unwrap();
        let mut run = self.run_writer.lock().unwrap();
        let mut entropy = self.entropy_writer.lock().unwrap();
        files.flush()?;
        strings.flush()?;
        history.flush()?;
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

        assert!(dir.path().join("metadata").join("carved_files.csv").exists());
        assert!(dir.path().join("metadata").join("string_artefacts.csv").exists());
        assert!(dir.path().join("metadata").join("browser_history.csv").exists());
        assert!(dir.path().join("metadata").join("run_summary.csv").exists());
        assert!(dir.path().join("metadata").join("entropy_regions.csv").exists());
    }
}
