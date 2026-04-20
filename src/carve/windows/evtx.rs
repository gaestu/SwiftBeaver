#[derive(Debug, Clone, serde::Serialize)]
pub struct EvtxArtefact {
    pub run_id: String,
    pub offset: u64,
    pub size: u64,
    pub first_chunk: u64,
    pub last_chunk: u64,
    pub record_count_estimate: u64,
    pub log_name: Option<String>,
}
