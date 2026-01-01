use std::fs::File;
use std::path::PathBuf;

use parquet::file::reader::{FileReader, SerializedFileReader};

use fastcarve::carve::CarvedFile;
use fastcarve::config;
use fastcarve::metadata::{self, EntropyRegion, MetadataBackendKind, RunSummary};
use fastcarve::parsers::browser::{
    BrowserCookieRecord, BrowserDownloadRecord, BrowserHistoryRecord,
};
use fastcarve::strings::artifacts::{ArtefactKind, StringArtefact};

#[test]
fn parquet_writes_expected_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let run_output_dir = tmp.path().join("run");
    std::fs::create_dir_all(&run_output_dir).expect("run dir");

    let loaded = config::load_config(None).expect("config");
    let cfg = loaded.config;

    let sink = metadata::build_sink(
        MetadataBackendKind::Parquet,
        &cfg,
        "run_001",
        "0.1.0",
        &loaded.config_hash,
        &PathBuf::from("evidence.dd"),
        "",
        &run_output_dir,
    )
    .expect("parquet sink");

    let file = CarvedFile {
        run_id: "run_001".to_string(),
        file_type: "jpeg".to_string(),
        path: "carved/jpeg_00000001.jpg".to_string(),
        extension: "jpg".to_string(),
        global_start: 10,
        global_end: 19,
        size: 10,
        md5: None,
        sha256: None,
        validated: true,
        truncated: false,
        errors: Vec::new(),
        pattern_id: Some("jpeg_soi".to_string()),
    };
    sink.record_file(&file).expect("record file");

    let artefact = StringArtefact {
        run_id: "run_001".to_string(),
        artefact_kind: ArtefactKind::Url,
        content: "https://example.com/path?q=1".to_string(),
        encoding: "ascii".to_string(),
        global_start: 100,
        global_end: 123,
    };
    sink.record_string(&artefact).expect("record url");

    let visit_time = chrono::DateTime::from_timestamp(1_600_000_000, 0).map(|dt| dt.naive_utc());
    let record = BrowserHistoryRecord {
        run_id: "run_001".to_string(),
        browser: "chrome".to_string(),
        profile: "Default".to_string(),
        url: "https://example.com/".to_string(),
        title: Some("Example".to_string()),
        visit_time,
        visit_source: Some("typed".to_string()),
        source_file: PathBuf::from("carved/history.sqlite"),
    };
    sink.record_history(&record).expect("record history");

    let cookie = BrowserCookieRecord {
        run_id: "run_001".to_string(),
        browser: "chrome".to_string(),
        profile: "Default".to_string(),
        host: "example.com".to_string(),
        name: "sid".to_string(),
        value: Some("abc123".to_string()),
        path: Some("/".to_string()),
        expires_utc: visit_time,
        last_access_utc: None,
        creation_utc: None,
        is_secure: Some(true),
        is_http_only: Some(true),
        source_file: PathBuf::from("carved/Cookies"),
    };
    sink.record_cookie(&cookie).expect("record cookie");

    let download = BrowserDownloadRecord {
        run_id: "run_001".to_string(),
        browser: "chrome".to_string(),
        profile: "Default".to_string(),
        url: Some("https://example.com/file.zip".to_string()),
        target_path: Some("/tmp/file.zip".to_string()),
        start_time: visit_time,
        end_time: None,
        total_bytes: Some(123),
        state: Some("1".to_string()),
        source_file: PathBuf::from("carved/History"),
    };
    sink.record_download(&download).expect("record download");
    let summary = RunSummary {
        run_id: "run_001".to_string(),
        bytes_scanned: 1024,
        chunks_processed: 1,
        hits_found: 2,
        files_carved: 1,
        string_spans: 3,
        artefacts_extracted: 4,
    };
    sink.record_run_summary(&summary).expect("record summary");
    let entropy = EntropyRegion {
        run_id: "run_001".to_string(),
        global_start: 0,
        global_end: 4095,
        entropy: 7.8,
        window_size: 4096,
    };
    sink.record_entropy(&entropy).expect("record entropy");

    sink.flush().expect("flush");

    let parquet_dir = run_output_dir.join("parquet");
    let files_path = parquet_dir.join("files_jpeg.parquet");
    let urls_path = parquet_dir.join("artefacts_urls.parquet");
    let history_path = parquet_dir.join("browser_history.parquet");
    let cookies_path = parquet_dir.join("browser_cookies.parquet");
    let downloads_path = parquet_dir.join("browser_downloads.parquet");
    let summary_path = parquet_dir.join("run_summary.parquet");
    let entropy_path = parquet_dir.join("entropy_regions.parquet");

    assert!(files_path.exists());
    assert!(urls_path.exists());
    assert!(history_path.exists());
    assert!(cookies_path.exists());
    assert!(downloads_path.exists());
    assert!(summary_path.exists());
    assert!(entropy_path.exists());

    assert_eq!(count_rows(&files_path), 1);
    assert_eq!(count_rows(&urls_path), 1);
    assert_eq!(count_rows(&history_path), 1);
    assert_eq!(count_rows(&cookies_path), 1);
    assert_eq!(count_rows(&downloads_path), 1);
    assert_eq!(count_rows(&summary_path), 1);
    assert_eq!(count_rows(&entropy_path), 1);

    assert_has_column(&files_path, "evidence_sha256");
    assert_has_column(&urls_path, "evidence_sha256");
    assert_has_column(&history_path, "evidence_sha256");
    assert_has_column(&cookies_path, "evidence_sha256");
    assert_has_column(&downloads_path, "evidence_sha256");
    assert_has_column(&summary_path, "evidence_sha256");
    assert_has_column(&entropy_path, "evidence_sha256");
    assert_has_column(&entropy_path, "entropy");
}

fn count_rows(path: &PathBuf) -> usize {
    let file = File::open(path).expect("open parquet");
    let reader = SerializedFileReader::new(file).expect("parquet reader");
    reader.get_row_iter(None).expect("row iter").count()
}

fn assert_has_column(path: &PathBuf, column: &str) {
    let file = File::open(path).expect("open parquet");
    let reader = SerializedFileReader::new(file).expect("parquet reader");
    let schema = reader
        .metadata()
        .file_metadata()
        .schema_descr()
        .root_schema();
    let columns: Vec<&str> = schema
        .get_fields()
        .iter()
        .map(|field| field.name())
        .collect();
    assert!(
        columns.contains(&column),
        "expected column {column} in {} got {:?}",
        path.display(),
        columns
    );
}
