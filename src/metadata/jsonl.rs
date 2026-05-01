use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;

use crate::carve::CarvedFile;
use crate::carve::bek::BitlockerBekRecord;
use crate::carve::windows::WindowsArtefactRecord;
use crate::metadata::windows::{WindowsArtefactFlatRow, flatten_windows_artefact};
use crate::metadata::{EntropyRegion, MetadataError, MetadataSink, RunSummary};
use crate::parsers::browser::{
    BrowserCookieRecord as CookieRecord, BrowserDownloadRecord as DownloadRecord,
};
use crate::strings::artifacts::StringArtefact;

pub struct JsonlSink {
    tool_version: String,
    config_hash: String,
    evidence_path: String,
    evidence_sha256: String,
    files_writer: Mutex<BufWriter<File>>,
    bitlocker_bek_writer: Mutex<BufWriter<File>>,
    strings_writer: Mutex<BufWriter<File>>,
    windows_writer: Mutex<BufWriter<File>>,
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
struct BitlockerBekJsonlRecord<'a> {
    #[serde(flatten)]
    record: &'a BitlockerBekRecord,
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
struct WindowsArtefactJsonlRecord<'a> {
    #[serde(flatten)]
    record: &'a WindowsArtefactFlatRow,
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
        let bitlocker_bek_path = meta_dir.join("bitlocker_bek.jsonl");
        let strings_path = meta_dir.join("string_artefacts.jsonl");
        let windows_path = meta_dir.join("windows_artefacts.jsonl");
        let history_path = meta_dir.join("browser_history.jsonl");
        let cookies_path = meta_dir.join("browser_cookies.jsonl");
        let downloads_path = meta_dir.join("browser_downloads.jsonl");
        let run_path = meta_dir.join("run_summary.jsonl");
        let entropy_path = meta_dir.join("entropy_regions.jsonl");
        let files_file = File::create(files_path)?;
        let bitlocker_bek_file = File::create(bitlocker_bek_path)?;
        let strings_file = File::create(strings_path)?;
        let windows_file = File::create(windows_path)?;
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
            bitlocker_bek_writer: Mutex::new(BufWriter::new(bitlocker_bek_file)),
            strings_writer: Mutex::new(BufWriter::new(strings_file)),
            windows_writer: Mutex::new(BufWriter::new(windows_file)),
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
        let mut guard = self
            .files_writer
            .lock()
            .map_err(|_| MetadataError::Other("files writer lock poisoned".into()))?;
        serde_json::to_writer(&mut *guard, &record)?;
        guard.write_all(b"\n")?;
        Ok(())
    }

    fn record_bitlocker_bek(&self, record: &BitlockerBekRecord) -> Result<(), MetadataError> {
        let record = BitlockerBekJsonlRecord {
            record,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self
            .bitlocker_bek_writer
            .lock()
            .map_err(|_| MetadataError::Other("bitlocker bek writer lock poisoned".into()))?;
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
        let mut guard = self
            .strings_writer
            .lock()
            .map_err(|_| MetadataError::Other("strings writer lock poisoned".into()))?;
        serde_json::to_writer(&mut *guard, &record)?;
        guard.write_all(b"\n")?;
        Ok(())
    }

    fn record_windows_artefact(&self, record: &WindowsArtefactRecord) -> Result<(), MetadataError> {
        let flat = flatten_windows_artefact(record)?;
        let record = WindowsArtefactJsonlRecord {
            record: &flat,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self
            .windows_writer
            .lock()
            .map_err(|_| MetadataError::Other("windows writer lock poisoned".into()))?;
        serde_json::to_writer(&mut *guard, &record)?;
        guard.write_all(b"\n")?;
        Ok(())
    }

    fn record_history(
        &self,
        record: &crate::parsers::browser::BrowserHistoryRecord,
    ) -> Result<(), MetadataError> {
        let record = BrowserHistoryRecord {
            record,
            tool_version: &self.tool_version,
            config_hash: &self.config_hash,
            evidence_path: &self.evidence_path,
            evidence_sha256: &self.evidence_sha256,
        };
        let mut guard = self
            .history_writer
            .lock()
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
        let mut guard = self
            .cookies_writer
            .lock()
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
        let mut guard = self
            .downloads_writer
            .lock()
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
        let mut guard = self
            .run_writer
            .lock()
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
        let mut guard = self
            .entropy_writer
            .lock()
            .map_err(|_| MetadataError::Other("entropy writer lock poisoned".into()))?;
        serde_json::to_writer(&mut *guard, &record)?;
        guard.write_all(b"\n")?;
        Ok(())
    }

    fn flush(&self) -> Result<(), MetadataError> {
        let mut files = self
            .files_writer
            .lock()
            .map_err(|_| MetadataError::Other("files writer lock poisoned".into()))?;
        let mut bitlocker_bek = self
            .bitlocker_bek_writer
            .lock()
            .map_err(|_| MetadataError::Other("bitlocker bek writer lock poisoned".into()))?;
        let mut strings = self
            .strings_writer
            .lock()
            .map_err(|_| MetadataError::Other("strings writer lock poisoned".into()))?;
        let mut windows = self
            .windows_writer
            .lock()
            .map_err(|_| MetadataError::Other("windows writer lock poisoned".into()))?;
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
        bitlocker_bek.flush()?;
        strings.flush()?;
        windows.flush()?;
        history.flush()?;
        cookies.flush()?;
        downloads.flush()?;
        run.flush()?;
        entropy.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::JsonlSink;
    use crate::carve::windows::{
        EvtxArtefact, LnkArtefact, PrefetchArtefact, RegistryHiveArtefact, WindowsArtefactRecord,
    };
    use crate::metadata::MetadataSink;
    use std::path::Path;

    #[test]
    fn windows_artefact_variants_serialize_to_jsonl() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sink = JsonlSink::new(
            "run_windows",
            "0.1.0",
            "hash",
            Path::new("/evidence.dd"),
            "",
            dir.path(),
        )
        .expect("jsonl sink");

        let ts = chrono::DateTime::from_timestamp(1_700_000_000, 0)
            .expect("timestamp")
            .naive_utc();
        let records = vec![
            WindowsArtefactRecord::Lnk(LnkArtefact {
                run_id: "run_windows".to_string(),
                offset: 1,
                size: 76,
                target_path: Some(r"C:\Temp\a.txt".to_string()),
                working_dir: Some(r"C:\Temp".to_string()),
                creation_time: Some(ts),
                access_time: Some(ts),
                write_time: Some(ts),
                file_size: 42,
                volume_serial: Some("ABCD-1234".to_string()),
                local_base_path: Some(r"C:\Temp\a.txt".to_string()),
                network_path: None,
            }),
            WindowsArtefactRecord::Prefetch(PrefetchArtefact {
                run_id: "run_windows".to_string(),
                offset: 2,
                size: 512,
                executable_name: "CMD.EXE".to_string(),
                prefetch_hash: "00112233".to_string(),
                run_count: 3,
                last_run_times: vec![ts],
                volume_paths: vec![r"\Device\HarddiskVolume1".to_string()],
                volume_paths_truncated: false,
                referenced_files: Some(vec![r"C:\Windows\System32\cmd.exe".to_string()]),
                version: 30,
            }),
            WindowsArtefactRecord::Evtx(EvtxArtefact {
                run_id: "run_windows".to_string(),
                offset: 3,
                size: 4096,
                first_chunk: 0,
                last_chunk: 10,
                record_count_estimate: Some(128),
                log_name: Some("Security".to_string()),
            }),
            WindowsArtefactRecord::RegistryHive(RegistryHiveArtefact {
                run_id: "run_windows".to_string(),
                offset: 4,
                size: 4096,
                timestamp: Some(ts),
                hive_name: Some("SOFTWARE".to_string()),
                hive_type: Some("SOFTWARE".to_string()),
                root_key_name: Some("Microsoft".to_string()),
            }),
        ];

        for record in &records {
            sink.record_windows_artefact(record)
                .expect("record windows artefact");
        }
        sink.flush().expect("flush");

        let content =
            std::fs::read_to_string(dir.path().join("metadata").join("windows_artefacts.jsonl"))
                .expect("read jsonl");
        assert!(content.contains("\"artefact_type\":\"lnk\""));
        assert!(content.contains("\"artefact_type\":\"prefetch\""));
        assert!(content.contains("\"volume_paths_truncated\":false"));
        assert!(content.contains("\"artefact_type\":\"evtx\""));
        assert!(content.contains("\"artefact_type\":\"registry\""));
        assert!(content.contains("\"evidence_sha256\":\"\""));
    }
}
