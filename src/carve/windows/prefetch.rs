#[derive(Debug, Clone, serde::Serialize)]
pub struct PrefetchArtefact {
    pub run_id: String,
    pub offset: u64,
    pub size: u64,
    pub executable_name: String,
    pub prefetch_hash: String,
    pub run_count: u32,
    pub last_run_times: Vec<chrono::NaiveDateTime>,
    pub volume_paths: Vec<String>,
    pub referenced_files: Vec<String>,
    pub version: u8,
}
