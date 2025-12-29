use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;

use crate::carve::CarvedFile;
use crate::metadata::{EntropyRegion, MetadataError, MetadataSink, RunSummary};
use crate::parsers::browser::{BrowserCookieRecord as CookieRecord, BrowserDownloadRecord as DownloadRecord};
use crate::strings::artifacts::StringArtefact;

pub struct JsonlSink {
    tool_version: String,
    config_hash: String,
    evidence_path: String,
    evidence_sha256: String,
    files_writer: Mutex<BufWriter<File>>,
    strings_writer: Mutex<BufWriter<File>>,
    history_writer: Mutex<BufWriter<File>>,
    cookies_writer: Mutex<BufWriter<File>>,
    downloads_writer: Mutex<BufWriter<File>>,
    run_writer: Mutex<BufWriter<File>>,
    entropy_writer: Mutex<BufWriter<File>>,
}

#[derive(Serialize)]
struct CarvedFileRecord<'a> {
    #[serde(flatten)]
    file: &'a CarvedFile,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

#[derive(Serialize)]
struct StringArtefactRecord<'a> {
    #[serde(flatten)]
    artefact: &'a StringArtefact,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

#[derive(Serialize)]
struct BrowserHistoryRecord<'a> {
    #[serde(flatten)]
    record: &'a crate::parsers::browser::BrowserHistoryRecord,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

#[derive(Serialize)]
struct BrowserCookieRecord<'a> {
    #[serde(flatten)]
    record: &'a CookieRecord,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

#[derive(Serialize)]
struct BrowserDownloadRecord<'a> {
    #[serde(flatten)]
    record: &'a DownloadRecord,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

#[derive(Serialize)]
struct RunSummaryRecord<'a> {
    #[serde(flatten)]
    summary: &'a RunSummary,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

#[derive(Serialize)]
struct EntropyRegionRecord<'a> {
    #[serde(flatten)]
    region: &'a EntropyRegion,
    tool_version: &'a str,
    config_hash: &'a str,
    evidence_path: &'a str,
    evidence_sha256: &'a str,
}

impl JsonlSink {
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
        let files_path = meta_dir.join("carved_files.jsonl");
        let strings_path = meta_dir.join("string_artefacts.jsonl");
        let history_path = meta_dir.join("browser_history.jsonl");
        let cookies_path = meta_dir.join("browser_cookies.jsonl");
        let downloads_path = meta_dir.join("browser_downloads.jsonl");
        let run_path = meta_dir.join("run_summary.jsonl");
        let entropy_path = meta_dir.join("entropy_regions.jsonl");
        let files_file = File::create(files_path)?;
        let strings_file = File::create(strings_path)?;
        let history_file = File::create(history_path)?;
        let cookies_file = File::create(cookies_path)?;
        let downloads_file = File::create(downloads_path)?;
        let run_file = File::create(run_path)?;
        let entropy_file = File::create(entropy_path)?;
        Ok(Self {
            tool_version: tool_version.to_string(),
            config_hash: config_hash.to_string(),
            evidence_path: evidence_path.to_string_lossy().to_string(),
            evidence_sha256: evidence_sha256.to_string(),
            files_writer: Mutex::new(BufWriter::new(files_file)),
            strings_writer: Mutex::new(BufWriter::new(strings_file)),
            history_writer: Mutex::new(BufWriter::new(history_file)),
            cookies_writer: Mutex::new(BufWriter::new(cookies_file)),
            downloads_writer: Mutex::new(BufWriter::new(downloads_file)),
            run_writer: Mutex::new(BufWriter::new(run_file)),
            entropy_writer: Mutex::new(BufWriter::new(entropy_file)),
        })
    }
}

impl MetadataSink for JsonlSink {
    fn record_file(&self, file: &CarvedFile) -> Result<(), MetadataError> {
        let record = CarvedFileRecord {
            file,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self.files_writer.lock()
            .map_err(|_| MetadataError::Other("files writer lock poisoned".into()))?;
        serde_json::to_writer(&mut *guard, &record)?;
        guard.write_all(b"\n")?;
        Ok(())
    }

    fn record_string(&self, artefact: &StringArtefact) -> Result<(), MetadataError> {
        let record = StringArtefactRecord {
            artefact,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self.strings_writer.lock()
            .map_err(|_| MetadataError::Other("strings writer lock poisoned".into()))?;
        serde_json::to_writer(&mut *guard, &record)?;
        guard.write_all(b"\n")?;
        Ok(())
    }

    fn record_history(&self, record: &crate::parsers::browser::BrowserHistoryRecord) -> Result<(), MetadataError> {
        let record = BrowserHistoryRecord {
            record,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self.history_writer.lock()
            .map_err(|_| MetadataError::Other("history writer lock poisoned".into()))?;
        serde_json::to_writer(&mut *guard, &record)?;
        guard.write_all(b"\n")?;
        Ok(())
    }

    fn record_cookie(&self, record: &CookieRecord) -> Result<(), MetadataError> {
        let record = BrowserCookieRecord {
            record,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self.cookies_writer.lock()
            .map_err(|_| MetadataError::Other("cookies writer lock poisoned".into()))?;
        serde_json::to_writer(&mut *guard, &record)?;
        guard.write_all(b"\n")?;
        Ok(())
    }

    fn record_download(&self, record: &DownloadRecord) -> Result<(), MetadataError> {
        let record = BrowserDownloadRecord {
            record,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self.downloads_writer.lock()
            .map_err(|_| MetadataError::Other("downloads writer lock poisoned".into()))?;
        serde_json::to_writer(&mut *guard, &record)?;
        guard.write_all(b"\n")?;
        Ok(())
    }

    fn record_run_summary(&self, summary: &RunSummary) -> Result<(), MetadataError> {
        let record = RunSummaryRecord {
            summary,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self.run_writer.lock()
            .map_err(|_| MetadataError::Other("run writer lock poisoned".into()))?;
        serde_json::to_writer(&mut *guard, &record)?;
        guard.write_all(b"\n")?;
        Ok(())
    }

    fn record_entropy(&self, region: &EntropyRegion) -> Result<(), MetadataError> {
        let record = EntropyRegionRecord {
            region,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self.entropy_writer.lock()
            .map_err(|_| MetadataError::Other("entropy writer lock poisoned".into()))?;
        serde_json::to_writer(&mut *guard, &record)?;
        guard.write_all(b"\n")?;
        Ok(())
    }

    fn flush(&self) -> Result<(), MetadataError> {
        let mut files = self.files_writer.lock()
            .map_err(|_| MetadataError::Other("files writer lock poisoned".into()))?;
        let mut strings = self.strings_writer.lock()
            .map_err(|_| MetadataError::Other("strings writer lock poisoned".into()))?;
        let mut history = self.history_writer.lock()
            .map_err(|_| MetadataError::Other("history writer lock poisoned".into()))?;
        let mut cookies = self.cookies_writer.lock()
            .map_err(|_| MetadataError::Other("cookies writer lock poisoned".into()))?;
        let mut downloads = self.downloads_writer.lock()
            .map_err(|_| MetadataError::Other("downloads writer lock poisoned".into()))?;
        let mut run = self.run_writer.lock()
            .map_err(|_| MetadataError::Other("run writer lock poisoned".into()))?;
        let mut entropy = self.entropy_writer.lock()
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
