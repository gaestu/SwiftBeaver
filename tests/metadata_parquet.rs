use std::fs::File;
use std::path::PathBuf;

use parquet::file::reader::{FileReader, SerializedFileReader};

use fastcarve::carve::CarvedFile;
use fastcarve::config;
use fastcarve::metadata::{self, MetadataBackendKind};
use fastcarve::parsers::browser::BrowserHistoryRecord;
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

    sink.flush().expect("flush");

    let parquet_dir = run_output_dir.join("parquet");
    let files_path = parquet_dir.join("files_jpeg.parquet");
    let urls_path = parquet_dir.join("artefacts_urls.parquet");
    let history_path = parquet_dir.join("browser_history.parquet");

    assert!(files_path.exists());
    assert!(urls_path.exists());
    assert!(history_path.exists());

    assert_eq!(count_rows(&files_path), 1);
    assert_eq!(count_rows(&urls_path), 1);
    assert_eq!(count_rows(&history_path), 1);
}

fn count_rows(path: &PathBuf) -> usize {
    let file = File::open(path).expect("open parquet");
    let reader = SerializedFileReader::new(file).expect("parquet reader");
    reader.get_row_iter(None).expect("row iter").count()
}
