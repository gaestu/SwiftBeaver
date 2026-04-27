use crate::carve::windows::WindowsArtefactRecord;
use crate::metadata::MetadataError;

#[derive(Debug, Clone, serde::Serialize)]
pub struct WindowsArtefactFlatRow {
    pub run_id: String,
    pub artefact_type: String,
    pub offset: u64,
    pub size: u64,
    pub target_path: Option<String>,
    pub working_dir: Option<String>,
    pub creation_time: Option<chrono::NaiveDateTime>,
    pub access_time: Option<chrono::NaiveDateTime>,
    pub write_time: Option<chrono::NaiveDateTime>,
    pub file_size: Option<u64>,
    pub volume_serial: Option<String>,
    pub local_base_path: Option<String>,
    pub network_path: Option<String>,
    pub executable_name: Option<String>,
    pub prefetch_hash: Option<String>,
    pub run_count: Option<u64>,
    pub last_run_times_json: Option<String>,
    pub volume_paths_json: Option<String>,
    pub volume_paths_truncated: Option<bool>,
    pub referenced_files_json: Option<String>,
    pub version: Option<u32>,
    pub first_chunk: Option<u64>,
    pub last_chunk: Option<u64>,
    pub record_count_estimate: Option<u64>,
    pub log_name: Option<String>,
    pub timestamp: Option<chrono::NaiveDateTime>,
    pub hive_name: Option<String>,
    pub hive_type: Option<String>,
    pub root_key_name: Option<String>,
}

fn serialize_prefetch_references(
    referenced_files: Option<&Vec<String>>,
) -> Result<Option<String>, MetadataError> {
    referenced_files
        .map(serde_json::to_string)
        .transpose()
        .map_err(Into::into)
}

pub fn flatten_windows_artefact(
    record: &WindowsArtefactRecord,
) -> Result<WindowsArtefactFlatRow, MetadataError> {
    match record {
        WindowsArtefactRecord::Lnk(artefact) => Ok(WindowsArtefactFlatRow {
            run_id: artefact.run_id.clone(),
            artefact_type: record.artefact_type().to_string(),
            offset: artefact.offset,
            size: artefact.size,
            target_path: artefact.target_path.clone(),
            working_dir: artefact.working_dir.clone(),
            creation_time: artefact.creation_time,
            access_time: artefact.access_time,
            write_time: artefact.write_time,
            file_size: Some(u64::from(artefact.file_size)),
            volume_serial: artefact.volume_serial.clone(),
            local_base_path: artefact.local_base_path.clone(),
            network_path: artefact.network_path.clone(),
            executable_name: None,
            prefetch_hash: None,
            run_count: None,
            last_run_times_json: None,
            volume_paths_json: None,
            volume_paths_truncated: None,
            referenced_files_json: None,
            version: None,
            first_chunk: None,
            last_chunk: None,
            record_count_estimate: None,
            log_name: None,
            timestamp: None,
            hive_name: None,
            hive_type: None,
            root_key_name: None,
        }),
        WindowsArtefactRecord::Prefetch(artefact) => Ok(WindowsArtefactFlatRow {
            run_id: artefact.run_id.clone(),
            artefact_type: record.artefact_type().to_string(),
            offset: artefact.offset,
            size: artefact.size,
            target_path: None,
            working_dir: None,
            creation_time: None,
            access_time: None,
            write_time: None,
            file_size: None,
            volume_serial: None,
            local_base_path: None,
            network_path: None,
            executable_name: Some(artefact.executable_name.clone()),
            prefetch_hash: Some(artefact.prefetch_hash.clone()),
            run_count: Some(u64::from(artefact.run_count)),
            last_run_times_json: Some(serde_json::to_string(&artefact.last_run_times)?),
            volume_paths_json: Some(serde_json::to_string(&artefact.volume_paths)?),
            volume_paths_truncated: Some(artefact.volume_paths_truncated),
            referenced_files_json: serialize_prefetch_references(
                artefact.referenced_files.as_ref(),
            )?,
            version: Some(u32::from(artefact.version)),
            first_chunk: None,
            last_chunk: None,
            record_count_estimate: None,
            log_name: None,
            timestamp: None,
            hive_name: None,
            hive_type: None,
            root_key_name: None,
        }),
        WindowsArtefactRecord::Evtx(artefact) => Ok(WindowsArtefactFlatRow {
            run_id: artefact.run_id.clone(),
            artefact_type: record.artefact_type().to_string(),
            offset: artefact.offset,
            size: artefact.size,
            target_path: None,
            working_dir: None,
            creation_time: None,
            access_time: None,
            write_time: None,
            file_size: None,
            volume_serial: None,
            local_base_path: None,
            network_path: None,
            executable_name: None,
            prefetch_hash: None,
            run_count: None,
            last_run_times_json: None,
            volume_paths_json: None,
            volume_paths_truncated: None,
            referenced_files_json: None,
            version: None,
            first_chunk: Some(artefact.first_chunk),
            last_chunk: Some(artefact.last_chunk),
            record_count_estimate: artefact.record_count_estimate,
            log_name: artefact.log_name.clone(),
            timestamp: None,
            hive_name: None,
            hive_type: None,
            root_key_name: None,
        }),
        WindowsArtefactRecord::RegistryHive(artefact) => Ok(WindowsArtefactFlatRow {
            run_id: artefact.run_id.clone(),
            artefact_type: record.artefact_type().to_string(),
            offset: artefact.offset,
            size: artefact.size,
            target_path: None,
            working_dir: None,
            creation_time: None,
            access_time: None,
            write_time: None,
            file_size: None,
            volume_serial: None,
            local_base_path: None,
            network_path: None,
            executable_name: None,
            prefetch_hash: None,
            run_count: None,
            last_run_times_json: None,
            volume_paths_json: None,
            volume_paths_truncated: None,
            referenced_files_json: None,
            version: None,
            first_chunk: None,
            last_chunk: None,
            record_count_estimate: None,
            log_name: None,
            timestamp: artefact.timestamp,
            hive_name: artefact.hive_name.clone(),
            hive_type: artefact.hive_type.clone(),
            root_key_name: artefact.root_key_name.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::flatten_windows_artefact;
    use crate::carve::windows::{PrefetchArtefact, WindowsArtefactRecord};

    #[test]
    fn prefetch_referenced_files_none_serializes_as_null() {
        let record = WindowsArtefactRecord::Prefetch(PrefetchArtefact {
            run_id: "run_windows".to_string(),
            offset: 2,
            size: 512,
            executable_name: "CMD.EXE".to_string(),
            prefetch_hash: "00112233".to_string(),
            run_count: 3,
            last_run_times: Vec::new(),
            volume_paths: Vec::new(),
            volume_paths_truncated: false,
            referenced_files: None,
            version: 30,
        });

        let flat = flatten_windows_artefact(&record).expect("flatten prefetch");

        assert_eq!(flat.referenced_files_json, None);
    }

    #[test]
    fn prefetch_referenced_files_preserves_serialized_values() {
        let record = WindowsArtefactRecord::Prefetch(PrefetchArtefact {
            run_id: "run_windows".to_string(),
            offset: 2,
            size: 512,
            executable_name: "CMD.EXE".to_string(),
            prefetch_hash: "00112233".to_string(),
            run_count: 3,
            last_run_times: Vec::new(),
            volume_paths: Vec::new(),
            volume_paths_truncated: true,
            referenced_files: Some(vec![r"C:\Windows\System32\cmd.exe".to_string()]),
            version: 30,
        });

        let flat = flatten_windows_artefact(&record).expect("flatten prefetch");

        assert_eq!(
            flat.referenced_files_json.as_deref(),
            Some(r#"["C:\\Windows\\System32\\cmd.exe"]"#)
        );
        assert_eq!(flat.volume_paths_truncated, Some(true));
    }
}
