use std::fs::File;
use std::path::PathBuf;

use parquet::file::reader::{FileReader, SerializedFileReader};

use swiftbeaver::carve::CarvedFile;
use swiftbeaver::carve::bek::BitlockerBekRecord;
use swiftbeaver::carve::windows::{
    EvtxArtefact, LnkArtefact, PrefetchArtefact, RegistryHiveArtefact, WindowsArtefactRecord,
};
use swiftbeaver::config;
use swiftbeaver::metadata::{self, EntropyRegion, MetadataBackendKind, RunSummary};
use swiftbeaver::parsers::browser::{
    BrowserCookieRecord, BrowserDownloadRecord, BrowserHistoryRecord,
};
use swiftbeaver::strings::artifacts::{ArtefactKind, StringArtefact};

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
        is_duplicate: false,
        duplicate_of_offset: None,
    };
    sink.record_file(&file).expect("record file");

    let bek_record = BitlockerBekRecord {
        run_id: "run_001".to_string(),
        global_start: 512,
        global_end: 667,
        size: 156,
        carved_path: "bek/bek_000000000200.bek".to_string(),
        key_identifier_guid: "00112233-4455-6677-8899-aabbccddeeff".to_string(),
        description: Some("ExternalKey".to_string()),
        key_data_length: 32,
        key_encryption_method: u32::MAX,
        modification_filetime: u64::MAX,
    };
    sink.record_bitlocker_bek(&bek_record)
        .expect("record bitlocker bek");

    let artefact = StringArtefact {
        run_id: "run_001".to_string(),
        artefact_kind: ArtefactKind::Url,
        content: "https://example.com/path?q=1".to_string(),
        encoding: "ascii".to_string(),
        global_start: 100,
        global_end: 123,
    };
    sink.record_string(&artefact).expect("record url");

    let bitlocker_artefact = StringArtefact {
        run_id: "run_001".to_string(),
        artefact_kind: ArtefactKind::BitlockerRecoveryPassword,
        content: "111111-222222-333333-444444-555555-666666-072765-720885".to_string(),
        encoding: "ascii".to_string(),
        global_start: 200,
        global_end: 254,
    };
    sink.record_string(&bitlocker_artefact)
        .expect("record bitlocker recovery password");

    let visit_time = chrono::DateTime::from_timestamp(1_600_000_000, 0).map(|dt| dt.naive_utc());
    let windows_records = vec![
        WindowsArtefactRecord::Lnk(LnkArtefact {
            run_id: "run_001".to_string(),
            offset: 2048,
            size: 512,
            target_path: Some(r"C:\Users\Alice\Desktop\report.txt".to_string()),
            working_dir: Some(r"C:\Users\Alice\Desktop".to_string()),
            creation_time: visit_time,
            access_time: visit_time,
            write_time: visit_time,
            file_size: 64,
            volume_serial: Some("ABCD-1234".to_string()),
            local_base_path: Some(r"C:\Users\Alice\Desktop\report.txt".to_string()),
            network_path: None,
        }),
        WindowsArtefactRecord::Prefetch(PrefetchArtefact {
            run_id: "run_001".to_string(),
            offset: 4096,
            size: 1024,
            executable_name: "CMD.EXE".to_string(),
            prefetch_hash: "00112233".to_string(),
            run_count: 7,
            last_run_times: visit_time.iter().copied().collect(),
            volume_paths: vec![r"\Device\HarddiskVolume1".to_string()],
            volume_paths_truncated: false,
            referenced_files: Some(vec![r"C:\Windows\System32\cmd.exe".to_string()]),
            version: 30,
        }),
        WindowsArtefactRecord::Evtx(EvtxArtefact {
            run_id: "run_001".to_string(),
            offset: 8192,
            size: 4096,
            first_chunk: 0,
            last_chunk: 3,
            record_count_estimate: Some(128),
            log_name: Some("Security".to_string()),
        }),
        // Regression for issue #80: a u64 record_count_estimate that is
        // larger than i64::MAX must NOT cause the row to be dropped.
        // The Parquet sink should preserve the row with the column NULL.
        WindowsArtefactRecord::Evtx(EvtxArtefact {
            run_id: "run_001".to_string(),
            offset: 16384,
            size: 4096,
            first_chunk: 0,
            last_chunk: 0,
            record_count_estimate: Some(u64::MAX),
            log_name: Some("Application".to_string()),
        }),
        WindowsArtefactRecord::RegistryHive(RegistryHiveArtefact {
            run_id: "run_001".to_string(),
            offset: 12288,
            size: 4096,
            timestamp: visit_time,
            hive_name: Some("SOFTWARE".to_string()),
            hive_type: Some("SOFTWARE".to_string()),
            root_key_name: Some("Microsoft".to_string()),
        }),
    ];
    for record in &windows_records {
        sink.record_windows_artefact(record)
            .expect("record windows artefact");
    }

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
        files_rejected: 0,
        files_prevalidation_rejected: 0,
        files_capped: 0,
        overlap_skipped: 0,
        string_spans: 3,
        artefacts_extracted: 4,
        duplicates_found: 0,
        duplicates_skipped: 0,
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

    // Explicitly drop sink to ensure all data is flushed and footers are written
    drop(sink);

    let parquet_dir = run_output_dir.join("parquet");
    let files_path = parquet_dir.join("files_jpeg.parquet");
    let bek_path = parquet_dir.join("artefacts_bitlocker_bek.parquet");
    let urls_path = parquet_dir.join("artefacts_urls.parquet");
    let bitlocker_path = parquet_dir.join("artefacts_bitlocker_recovery_passwords.parquet");
    let history_path = parquet_dir.join("browser_history.parquet");
    let cookies_path = parquet_dir.join("browser_cookies.parquet");
    let downloads_path = parquet_dir.join("browser_downloads.parquet");
    let windows_path = parquet_dir.join("windows_artefacts.parquet");
    let summary_path = parquet_dir.join("run_summary.parquet");
    let entropy_path = parquet_dir.join("entropy_regions.parquet");

    assert!(files_path.exists());
    assert!(bek_path.exists());
    assert!(urls_path.exists());
    assert!(bitlocker_path.exists());
    assert!(history_path.exists());
    assert!(cookies_path.exists());
    assert!(downloads_path.exists());
    assert!(windows_path.exists());
    assert!(summary_path.exists());
    assert!(entropy_path.exists());

    assert_eq!(count_rows(&files_path), 1);
    assert_eq!(count_rows(&bek_path), 1);
    assert_eq!(count_rows(&urls_path), 1);
    assert_eq!(count_rows(&bitlocker_path), 1);
    assert_eq!(count_rows(&history_path), 1);
    assert_eq!(count_rows(&cookies_path), 1);
    assert_eq!(count_rows(&downloads_path), 1);
    assert_eq!(count_rows(&windows_path), 5);
    assert_eq!(count_rows(&summary_path), 1);
    assert_eq!(count_rows(&entropy_path), 1);

    assert_has_column(&files_path, "evidence_sha256");
    assert_has_column(&bek_path, "key_identifier_guid");
    assert_has_column(&bek_path, "key_data_length");
    assert_has_column(&bek_path, "evidence_sha256");
    assert_has_column(&urls_path, "evidence_sha256");
    assert_has_column(&bitlocker_path, "recovery_password");
    assert_has_column(&bitlocker_path, "evidence_sha256");
    assert_has_column(&history_path, "evidence_sha256");
    assert_has_column(&cookies_path, "evidence_sha256");
    assert_has_column(&downloads_path, "evidence_sha256");
    assert_has_column(&windows_path, "artefact_type");
    assert_has_column(&windows_path, "volume_paths_truncated");
    assert_has_column(&windows_path, "evidence_sha256");
    assert_has_column(&summary_path, "evidence_sha256");
    assert_has_column(&entropy_path, "evidence_sha256");
    assert_has_column(&entropy_path, "entropy");

    // Regression for issue #80: the EVTX row with `record_count_estimate =
    // u64::MAX` must be present in the Parquet file, and the column must be
    // serialized as NULL rather than silently truncated to a misleading i64.
    assert_evtx_overflow_row_has_null_record_count(&windows_path);
}

fn assert_evtx_overflow_row_has_null_record_count(path: &PathBuf) {
    use parquet::record::Field;

    let file = File::open(path).expect("open parquet");
    let reader = SerializedFileReader::new(file).expect("parquet reader");
    let mut found_overflow_row = false;
    for row in reader.get_row_iter(None).expect("row iter") {
        let row = row.expect("row");
        let mut artefact_type: Option<String> = None;
        let mut log_name: Option<String> = None;
        let mut record_count_field: Option<Field> = None;
        for (name, field) in row.get_column_iter() {
            match name.as_str() {
                "artefact_type" => {
                    if let Field::Str(value) = field {
                        artefact_type = Some(value.clone());
                    }
                }
                "log_name" => {
                    if let Field::Str(value) = field {
                        log_name = Some(value.clone());
                    }
                }
                "record_count_estimate" => {
                    record_count_field = Some(field.clone());
                }
                _ => {}
            }
        }
        if artefact_type.as_deref() == Some("evtx") && log_name.as_deref() == Some("Application") {
            found_overflow_row = true;
            assert!(
                matches!(record_count_field, Some(Field::Null)),
                "expected record_count_estimate to be NULL on overflow row, got {:?}",
                record_count_field
            );
        }
    }
    assert!(
        found_overflow_row,
        "overflow EVTX row (log_name=Application) not found in windows_artefacts.parquet"
    );
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
