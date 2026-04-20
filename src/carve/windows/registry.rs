#[derive(Debug, Clone, serde::Serialize)]
pub struct RegistryHiveArtefact {
    pub run_id: String,
    pub offset: u64,
    pub size: u64,
    pub timestamp: Option<chrono::NaiveDateTime>,
    pub hive_name: Option<String>,
    pub hive_type: Option<String>,
    pub root_key_name: Option<String>,
}
