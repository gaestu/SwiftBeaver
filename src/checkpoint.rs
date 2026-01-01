use std::fs;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CheckpointState {
    pub version: u32,
    pub run_id: String,
    pub chunk_size: u64,
    pub overlap: u64,
    pub next_offset: u64,
    pub evidence_len: u64,
    pub created_at: String,
}

impl CheckpointState {
    pub fn new(
        run_id: &str,
        chunk_size: u64,
        overlap: u64,
        next_offset: u64,
        evidence_len: u64,
    ) -> Self {
        Self {
            version: 1,
            run_id: run_id.to_string(),
            chunk_size,
            overlap,
            next_offset,
            evidence_len,
            created_at: Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn load_checkpoint(path: &Path) -> Result<CheckpointState, CheckpointError> {
    let contents = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&contents)?)
}

pub fn save_checkpoint(path: &Path, state: &CheckpointState) -> Result<(), CheckpointError> {
    let contents = serde_json::to_string_pretty(state)?;
    fs::write(path, contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_checkpoint() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("checkpoint.json");
        let state = CheckpointState::new("run", 1024, 64, 2048, 4096);
        save_checkpoint(&path, &state).expect("save");
        let loaded = load_checkpoint(&path).expect("load");
        assert_eq!(loaded.run_id, "run");
        assert_eq!(loaded.next_offset, 2048);
        assert_eq!(loaded.evidence_len, 4096);
    }
}
