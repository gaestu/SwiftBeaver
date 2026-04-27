use swiftbeaver::carve::ico::IcoCarveHandler;
use swiftbeaver::carve::{CarveHandler, PreValidation};
use swiftbeaver::evidence::{EvidenceError, EvidenceSource};

struct SliceEvidence {
    data: Vec<u8>,
}

impl EvidenceSource for SliceEvidence {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, EvidenceError> {
        let offset = offset as usize;
        if offset >= self.data.len() {
            return Ok(0);
        }
        let available = self.data.len() - offset;
        let to_copy = available.min(buf.len());
        buf[..to_copy].copy_from_slice(&self.data[offset..offset + to_copy]);
        Ok(to_copy)
    }
}

fn bmp_payload() -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&40u32.to_le_bytes());
    payload.extend_from_slice(&16i32.to_le_bytes());
    payload.extend_from_slice(&32i32.to_le_bytes());
    payload.extend_from_slice(&1u16.to_le_bytes());
    payload.extend_from_slice(&32u16.to_le_bytes());
    payload.extend_from_slice(&[0; 24]);
    payload.extend_from_slice(&[0xAA; 4]);
    payload
}

fn single_entry_ico_with_declared_size(declared_size: u32) -> Vec<u8> {
    let payload = bmp_payload();
    let mut data = Vec::new();
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&[16, 16, 0, 0]);
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&32u16.to_le_bytes());
    data.extend_from_slice(&declared_size.to_le_bytes());
    data.extend_from_slice(&22u32.to_le_bytes());
    data.extend_from_slice(&payload);
    data
}

#[test]
fn ico_prevalidation_accepts_plausible_directory() {
    let data = single_entry_ico_with_declared_size(bmp_payload().len() as u32);
    let evidence = SliceEvidence { data };
    let handler = IcoCarveHandler::new("ico".to_string(), 22, 10 * 1024 * 1024);

    let result = handler.pre_validate(&evidence, 0).expect("pre-validate");
    assert_eq!(result, PreValidation::Proceed);
}

#[test]
fn ico_prevalidation_rejects_implausible_count() {
    let evidence = SliceEvidence {
        data: vec![0x00, 0x00, 0x01, 0x00, 0xFF, 0xFF],
    };
    let handler = IcoCarveHandler::new("ico".to_string(), 22, 10 * 1024 * 1024);

    let result = handler.pre_validate(&evidence, 0).expect("pre-validate");
    assert!(matches!(result, PreValidation::Reject(_)));
}

// Note: declared-span / per-entry size limits are enforced in process_hit
// (see src/carve/ico.rs unit tests), not in pre_validate, which is kept
// header-only for performance.
