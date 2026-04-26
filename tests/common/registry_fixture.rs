//! Build minimal synthetic Windows Registry hive (`regf`) byte blobs for tests.

const BASE_BLOCK_SIZE: usize = 4096;
const HBIN_SIZE: usize = 4096;
const REGF_MAGIC: &[u8; 4] = b"regf";
const HBIN_MAGIC: &[u8; 4] = b"hbin";

/// FILETIME for 2024-01-01T00:00:00Z (matches the Python golden generator).
pub const FILETIME_2024_01_01: u64 = 133_485_984_000_000_000;

#[allow(dead_code)]
pub fn build_registry_hive(embedded_name: &str, root_name: &str) -> Vec<u8> {
    build_registry_hive_with_type_and_seq(embedded_name, root_name, 0, 1, 1)
}

#[allow(dead_code)]
pub fn build_registry_hive_with_seq(
    embedded_name: &str,
    root_name: &str,
    primary_seq: u32,
    secondary_seq: u32,
) -> Vec<u8> {
    build_registry_hive_with_type_and_seq(embedded_name, root_name, 0, primary_seq, secondary_seq)
}

#[allow(dead_code)]
pub fn build_registry_log_hive_with_seq(
    embedded_name: &str,
    root_name: &str,
    primary_seq: u32,
    secondary_seq: u32,
) -> Vec<u8> {
    build_registry_hive_with_type_and_seq(embedded_name, root_name, 1, primary_seq, secondary_seq)
}

fn build_registry_hive_with_type_and_seq(
    embedded_name: &str,
    root_name: &str,
    file_type: u32,
    primary_seq: u32,
    secondary_seq: u32,
) -> Vec<u8> {
    let mut buf = vec![0u8; BASE_BLOCK_SIZE + HBIN_SIZE];

    // ---- base block ----
    buf[0..4].copy_from_slice(REGF_MAGIC);
    buf[0x04..0x08].copy_from_slice(&primary_seq.to_le_bytes());
    buf[0x08..0x0C].copy_from_slice(&secondary_seq.to_le_bytes());
    buf[0x0C..0x14].copy_from_slice(&FILETIME_2024_01_01.to_le_bytes());
    buf[0x14..0x18].copy_from_slice(&1u32.to_le_bytes()); // major
    buf[0x18..0x1C].copy_from_slice(&5u32.to_le_bytes()); // minor
    buf[0x1C..0x20].copy_from_slice(&file_type.to_le_bytes());
    buf[0x20..0x24].copy_from_slice(&1u32.to_le_bytes()); // file_format
    buf[0x24..0x28].copy_from_slice(&0x20u32.to_le_bytes()); // root_cell_offset
    buf[0x28..0x2C].copy_from_slice(&(HBIN_SIZE as u32).to_le_bytes());
    buf[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes()); // clustering

    // Embedded filename UTF-16LE at 0x30, 64 bytes max.
    let mut pos = 0x30;
    for unit in embedded_name.encode_utf16().take(32) {
        buf[pos..pos + 2].copy_from_slice(&unit.to_le_bytes());
        pos += 2;
    }

    // ---- hbin ----
    let hbin = BASE_BLOCK_SIZE;
    buf[hbin..hbin + 4].copy_from_slice(HBIN_MAGIC);
    buf[hbin + 0x04..hbin + 0x08].copy_from_slice(&0u32.to_le_bytes());
    buf[hbin + 0x08..hbin + 0x0C].copy_from_slice(&(HBIN_SIZE as u32).to_le_bytes());
    buf[hbin + 0x14..hbin + 0x1C].copy_from_slice(&FILETIME_2024_01_01.to_le_bytes());

    // nk root cell at hbin + 0x20
    let cell = hbin + 0x20;
    let name_bytes = root_name.as_bytes();
    let nk_header_size: usize = 0x50;
    let body_len = nk_header_size + name_bytes.len();
    let total = 4 + body_len;
    let pad = (8 - (total % 8)) % 8;
    let cell_size = total + pad;
    let neg = -(cell_size as i32);

    buf[cell..cell + 4].copy_from_slice(&neg.to_le_bytes());
    buf[cell + 4..cell + 6].copy_from_slice(b"nk");
    buf[cell + 6..cell + 8].copy_from_slice(&0x002Cu16.to_le_bytes()); // root + ASCII
    buf[cell + 8..cell + 16].copy_from_slice(&FILETIME_2024_01_01.to_le_bytes());
    // name_length at body offset 0x48 = cell+4+0x48 = cell+0x4C
    buf[cell + 0x4C..cell + 0x4E].copy_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    let name_pos = cell + 4 + nk_header_size;
    buf[name_pos..name_pos + name_bytes.len()].copy_from_slice(name_bytes);
    buf
}
