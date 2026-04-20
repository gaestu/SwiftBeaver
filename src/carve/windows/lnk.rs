#[derive(Debug, Clone, serde::Serialize)]
pub struct LnkArtefact {
    pub run_id: String,
    pub offset: u64,
    pub size: u64,
    pub target_path: Option<String>,
    pub working_dir: Option<String>,
    pub creation_time: Option<chrono::NaiveDateTime>,
    pub access_time: Option<chrono::NaiveDateTime>,
    pub write_time: Option<chrono::NaiveDateTime>,
    pub file_size: u32,
    pub volume_serial: Option<String>,
    pub local_base_path: Option<String>,
    pub network_path: Option<String>,
}
