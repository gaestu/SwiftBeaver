//! BitLocker External Key (`.bek`) carving handler.

use crate::carve::{
    CarveError, CarveHandler, CarvedFile, ExtractionContext, PendingCarve, PostCarveMetadata,
    PreValidation, create_hashers, finalize_hashers, output_path, write_range,
};
use crate::evidence::EvidenceSource;
use crate::scanner::NormalizedHit;

const BEK_HEADER_LEN: usize = 48;
const METADATA_ENTRY_HEADER_LEN: usize = 8;
const MAX_REASONABLE_BEK_SIZE: u64 = 64 * 1024;
const HEADER_PATTERN_OFFSET: u64 = 4;

const BEK_VERSION_V1: u32 = 1;
const BEK_HEADER_SIZE_V1: u32 = 48;
const ENTRY_TYPE_STARTUP_KEY: u16 = 0x0006;
const ENTRY_TYPE_PROPERTY: u16 = 0x0000;
const VALUE_TYPE_KEY: u16 = 0x0001;
const VALUE_TYPE_UNICODE_STRING: u16 = 0x0002;
const VALUE_TYPE_EXTERNAL_KEY: u16 = 0x0009;
const SUPPORTED_ENTRY_VERSION: u16 = 1;
const STARTUP_KEY_DATA_LEN: usize = 32;

#[derive(Debug, Clone, serde::Serialize)]
pub struct BitlockerBekRecord {
    pub run_id: String,
    pub global_start: u64,
    pub global_end: u64,
    pub size: u64,
    pub carved_path: String,
    pub key_identifier_guid: String,
    pub description: Option<String>,
    pub key_data_length: u64,
    pub key_encryption_method: u32,
    pub modification_filetime: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedBek {
    metadata_size: u64,
    key_identifier_guid: String,
    description: Option<String>,
    key_data_length: u64,
    key_encryption_method: u32,
    modification_filetime: u64,
}

#[derive(Debug, Clone, Copy)]
struct MetadataEntry<'a> {
    entry_type: u16,
    value_type: u16,
    value_data: &'a [u8],
}

pub struct BekCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl BekCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }

    fn effective_max_size(&self) -> u64 {
        if self.max_size > 0 {
            self.max_size.min(MAX_REASONABLE_BEK_SIZE)
        } else {
            MAX_REASONABLE_BEK_SIZE
        }
    }
}

impl CarveHandler for BekCarveHandler {
    fn file_type(&self) -> &str {
        "bek"
    }

    fn is_fast(&self) -> bool {
        true
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let Some(candidate_start) = candidate_start_from_pattern_offset(offset) else {
            return Ok(PreValidation::Reject(
                "bek candidate before file start".to_string(),
            ));
        };
        match read_bek_metadata_size(evidence, candidate_start, self.effective_max_size())? {
            Some(_) => Ok(PreValidation::Proceed),
            None => Ok(PreValidation::Reject(
                "bek header validation failed".to_string(),
            )),
        }
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let Some(candidate_start) = candidate_start_from_pattern_offset(hit.global_offset) else {
            return Ok(None);
        };
        let Some(parsed) =
            read_and_parse_bek(ctx.evidence, candidate_start, self.effective_max_size())?
        else {
            return Ok(None);
        };
        if parsed.metadata_size < self.min_size {
            return Ok(None);
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            candidate_start,
        )?;
        let (mut md5, mut sha256) = create_hashers(&ctx.hash_config);
        let end = candidate_start
            .checked_add(parsed.metadata_size)
            .ok_or_else(|| CarveError::Invalid("bek size overflow".to_string()))?;
        let (written, eof_truncated, mut writer) = write_range(
            ctx,
            candidate_start,
            end,
            &full_path,
            md5.as_mut(),
            sha256.as_mut(),
        )?;
        if eof_truncated || written != parsed.metadata_size {
            writer.discard();
            return Ok(None);
        }

        let (md5_hex, sha256_hex) = finalize_hashers(md5, sha256);
        let global_end = candidate_start + written - 1;
        let file = CarvedFile {
            run_id: ctx.run_id.to_string(),
            file_type: self.file_type().to_string(),
            path: rel_path.clone(),
            extension: self.extension.clone(),
            global_start: candidate_start,
            global_end,
            size: written,
            md5: md5_hex,
            sha256: sha256_hex,
            validated: true,
            truncated: false,
            errors: Vec::new(),
            pattern_id: Some(hit.pattern_id.clone()),
            is_duplicate: false,
            duplicate_of_offset: None,
        };
        let record = BitlockerBekRecord {
            run_id: ctx.run_id.to_string(),
            global_start: candidate_start,
            global_end,
            size: written,
            carved_path: rel_path,
            key_identifier_guid: parsed.key_identifier_guid,
            description: parsed.description,
            key_data_length: parsed.key_data_length,
            key_encryption_method: parsed.key_encryption_method,
            modification_filetime: parsed.modification_filetime,
        };

        Ok(Some(PendingCarve::new(file, writer).with_post_metadata(
            vec![PostCarveMetadata::BitlockerBek(record)],
        )))
    }
}

fn candidate_start_from_pattern_offset(offset: u64) -> Option<u64> {
    offset.checked_sub(HEADER_PATTERN_OFFSET)
}

fn read_and_parse_bek(
    evidence: &dyn EvidenceSource,
    offset: u64,
    max_size: u64,
) -> Result<Option<ParsedBek>, CarveError> {
    let Some(metadata_size) = read_bek_metadata_size(evidence, offset, max_size)? else {
        return Ok(None);
    };

    let mut data = vec![0u8; metadata_size as usize];
    if !read_exact_from_evidence(evidence, offset, &mut data)? {
        return Ok(None);
    }

    Ok(parse_bek_bytes(&data))
}

fn read_bek_metadata_size(
    evidence: &dyn EvidenceSource,
    offset: u64,
    max_size: u64,
) -> Result<Option<u64>, CarveError> {
    if evidence.len().saturating_sub(offset) < BEK_HEADER_LEN as u64 {
        return Ok(None);
    }
    let mut header = [0u8; BEK_HEADER_LEN];
    if !read_exact_from_evidence(evidence, offset, &mut header)? {
        return Ok(None);
    }
    let metadata_size = u32_le(&header[0..4])?;
    let version = u32_le(&header[4..8])?;
    let header_size = u32_le(&header[8..12])?;
    let metadata_size_copy = u32_le(&header[12..16])?;
    if version != BEK_VERSION_V1
        || header_size != BEK_HEADER_SIZE_V1
        || metadata_size != metadata_size_copy
        || metadata_size < BEK_HEADER_SIZE_V1
    {
        return Ok(None);
    }
    let metadata_size = u64::from(metadata_size);
    if metadata_size > max_size || evidence.len().saturating_sub(offset) < metadata_size {
        return Ok(None);
    }

    Ok(Some(metadata_size))
}

fn read_exact_from_evidence(
    evidence: &dyn EvidenceSource,
    offset: u64,
    buf: &mut [u8],
) -> Result<bool, CarveError> {
    let mut filled = 0usize;
    while filled < buf.len() {
        let read_offset = offset
            .checked_add(filled as u64)
            .ok_or_else(|| CarveError::Invalid("bek read offset overflow".to_string()))?;
        let read = evidence
            .read_at(read_offset, &mut buf[filled..])
            .map_err(|err| CarveError::Evidence(err.to_string()))?;
        if read == 0 {
            return Ok(false);
        }
        filled += read;
    }
    Ok(true)
}

pub(crate) fn parse_bek_bytes(data: &[u8]) -> Option<ParsedBek> {
    if data.len() < BEK_HEADER_LEN {
        return None;
    }
    let metadata_size = u32_le(&data[0..4]).ok()?;
    let version = u32_le(&data[4..8]).ok()?;
    let header_size = u32_le(&data[8..12]).ok()?;
    let metadata_size_copy = u32_le(&data[12..16]).ok()?;
    if version != BEK_VERSION_V1
        || header_size != BEK_HEADER_SIZE_V1
        || metadata_size != metadata_size_copy
        || metadata_size < BEK_HEADER_SIZE_V1
        || metadata_size as usize != data.len()
    {
        return None;
    }

    let entries = parse_entries(&data[BEK_HEADER_LEN..], false)?;
    if entries
        .iter()
        .any(|entry| entry.entry_type == ENTRY_TYPE_PROPERTY && entry.value_type == VALUE_TYPE_KEY)
    {
        return None;
    }
    let mut startup_entries = entries
        .iter()
        .filter(|entry| entry.entry_type == ENTRY_TYPE_STARTUP_KEY);
    let startup_entry = startup_entries.next()?;
    if startup_entries.next().is_some() || startup_entry.value_type != VALUE_TYPE_EXTERNAL_KEY {
        return None;
    }
    parse_external_key(startup_entry).map(|mut parsed| {
        parsed.metadata_size = u64::from(metadata_size);
        parsed
    })
}

fn parse_entries<'a>(
    data: &'a [u8],
    require_property_type: bool,
) -> Option<Vec<MetadataEntry<'a>>> {
    let mut entries = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        let remaining = &data[offset..];
        if remaining.len() < METADATA_ENTRY_HEADER_LEN {
            return None;
        }
        if remaining[..METADATA_ENTRY_HEADER_LEN]
            .iter()
            .all(|byte| *byte == 0)
        {
            return remaining.iter().all(|byte| *byte == 0).then_some(entries);
        }

        let entry_size = usize::from(u16_le(&remaining[0..2]).ok()?);
        let entry_type = u16_le(&remaining[2..4]).ok()?;
        let value_type = u16_le(&remaining[4..6]).ok()?;
        let version = u16_le(&remaining[6..8]).ok()?;
        if entry_size < METADATA_ENTRY_HEADER_LEN
            || entry_size > remaining.len()
            || version != SUPPORTED_ENTRY_VERSION
            || (require_property_type && entry_type != ENTRY_TYPE_PROPERTY)
        {
            return None;
        }

        let value_data = &remaining[METADATA_ENTRY_HEADER_LEN..entry_size];
        entries.push(MetadataEntry {
            entry_type,
            value_type,
            value_data,
        });
        offset = offset.checked_add(entry_size)?;
    }

    Some(entries)
}

fn parse_external_key(entry: &MetadataEntry<'_>) -> Option<ParsedBek> {
    if entry.value_type != VALUE_TYPE_EXTERNAL_KEY || entry.value_data.len() < 24 {
        return None;
    }
    let key_identifier_guid = format_guid(&entry.value_data[0..16]);
    let modification_filetime = u64_le(&entry.value_data[16..24]).ok()?;
    let properties = parse_entries(&entry.value_data[24..], true)?;
    let mut description = None;
    let mut key_data_length = None;
    let mut key_encryption_method = None;

    for property in properties {
        match property.value_type {
            VALUE_TYPE_KEY => {
                if key_data_length.is_some() || key_encryption_method.is_some() {
                    return None;
                }
                if property.value_data.len() != 4 + STARTUP_KEY_DATA_LEN {
                    return None;
                }
                key_encryption_method = Some(u32_le(&property.value_data[0..4]).ok()?);
                key_data_length = Some((property.value_data.len() - 4) as u64);
            }
            VALUE_TYPE_UNICODE_STRING => {
                if description.is_some() {
                    return None;
                }
                description = parse_utf16le_nul_string(property.value_data);
            }
            _ => {}
        }
    }

    Some(ParsedBek {
        metadata_size: 0,
        key_identifier_guid,
        description,
        key_data_length: key_data_length?,
        key_encryption_method: key_encryption_method?,
        modification_filetime,
    })
}

fn parse_utf16le_nul_string(data: &[u8]) -> Option<String> {
    if !data.len().is_multiple_of(2) {
        return None;
    }
    let mut units = Vec::with_capacity(data.len() / 2);
    for chunk in data.chunks_exact(2) {
        let unit = u16::from_le_bytes([chunk[0], chunk[1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16(&units).ok()
}

fn format_guid(data: &[u8]) -> String {
    let d1 = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let d2 = u16::from_le_bytes([data[4], data[5]]);
    let d3 = u16::from_le_bytes([data[6], data[7]]);
    format!(
        "{d1:08x}-{d2:04x}-{d3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15]
    )
}

fn u16_le(data: &[u8]) -> Result<u16, CarveError> {
    let bytes: [u8; 2] = data
        .try_into()
        .map_err(|_| CarveError::Invalid("u16 slice length mismatch".to_string()))?;
    Ok(u16::from_le_bytes(bytes))
}

fn u32_le(data: &[u8]) -> Result<u32, CarveError> {
    let bytes: [u8; 4] = data
        .try_into()
        .map_err(|_| CarveError::Invalid("u32 slice length mismatch".to_string()))?;
    Ok(u32::from_le_bytes(bytes))
}

fn u64_le(data: &[u8]) -> Result<u64, CarveError> {
    let bytes: [u8; 8] = data
        .try_into()
        .map_err(|_| CarveError::Invalid("u64 slice length mismatch".to_string()))?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::{VALUE_TYPE_EXTERNAL_KEY, parse_bek_bytes};

    fn metadata_entry(entry_type: u16, value_type: u16, value: &[u8]) -> Vec<u8> {
        let size = 8u16 + u16::try_from(value.len()).expect("entry size");
        let mut entry = Vec::new();
        entry.extend_from_slice(&size.to_le_bytes());
        entry.extend_from_slice(&entry_type.to_le_bytes());
        entry.extend_from_slice(&value_type.to_le_bytes());
        entry.extend_from_slice(&1u16.to_le_bytes());
        entry.extend_from_slice(value);
        entry
    }

    fn key_property(key_len: usize) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&vec![0xBB; key_len]);
        metadata_entry(0, 1, &body)
    }

    fn description_property(value: &str) -> Vec<u8> {
        let mut body = Vec::new();
        for unit in value.encode_utf16() {
            body.extend_from_slice(&unit.to_le_bytes());
        }
        body.extend_from_slice(&0u16.to_le_bytes());
        metadata_entry(0, 2, &body)
    }

    fn valid_bek() -> Vec<u8> {
        valid_bek_with_properties(&[key_property(32)])
    }

    fn valid_bek_with_properties(properties: &[Vec<u8>]) -> Vec<u8> {
        let mut external = vec![0u8; 24];
        external[0..16].copy_from_slice(&[
            0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD,
            0xEE, 0xFF,
        ]);
        external[16..24].copy_from_slice(&1234u64.to_le_bytes());
        for property in properties {
            external.extend_from_slice(property);
        }
        let startup = metadata_entry(0x0006, VALUE_TYPE_EXTERNAL_KEY, &external);
        let metadata_size = 48u32 + u32::try_from(startup.len()).expect("metadata size");
        let mut header = vec![0u8; 48];
        header[0..4].copy_from_slice(&metadata_size.to_le_bytes());
        header[4..8].copy_from_slice(&1u32.to_le_bytes());
        header[8..12].copy_from_slice(&48u32.to_le_bytes());
        header[12..16].copy_from_slice(&metadata_size.to_le_bytes());
        [header, startup].concat()
    }

    #[test]
    fn parses_valid_bek() {
        let parsed = parse_bek_bytes(&valid_bek()).expect("valid bek");
        assert_eq!(
            parsed.key_identifier_guid,
            "00112233-4455-6677-8899-aabbccddeeff"
        );
        assert_eq!(parsed.key_data_length, 32);
        assert_eq!(parsed.modification_filetime, 1234);
    }

    #[test]
    fn rejects_non_32_byte_key() {
        let mut external = vec![0u8; 24];
        external.extend_from_slice(&key_property(31));
        let startup = metadata_entry(0x0006, VALUE_TYPE_EXTERNAL_KEY, &external);
        let metadata_size = 48u32 + u32::try_from(startup.len()).expect("metadata size");
        let mut header = vec![0u8; 48];
        header[0..4].copy_from_slice(&metadata_size.to_le_bytes());
        header[4..8].copy_from_slice(&1u32.to_le_bytes());
        header[8..12].copy_from_slice(&48u32.to_le_bytes());
        header[12..16].copy_from_slice(&metadata_size.to_le_bytes());
        assert!(parse_bek_bytes(&[header, startup].concat()).is_none());
    }

    #[test]
    fn parses_optional_utf16le_description() {
        let parsed = parse_bek_bytes(&valid_bek_with_properties(&[
            description_property("ExternalKey"),
            key_property(32),
        ]))
        .expect("valid bek with description");
        assert_eq!(parsed.description.as_deref(), Some("ExternalKey"));
    }

    #[test]
    fn rejects_too_short_header() {
        assert!(parse_bek_bytes(&[0u8; 47]).is_none());
    }

    #[test]
    fn rejects_unsupported_version_and_header_size() {
        let mut bek = valid_bek();
        bek[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert!(parse_bek_bytes(&bek).is_none());

        let mut bek = valid_bek();
        bek[8..12].copy_from_slice(&64u32.to_le_bytes());
        assert!(parse_bek_bytes(&bek).is_none());
    }

    #[test]
    fn rejects_metadata_size_smaller_than_header() {
        let mut bek = valid_bek();
        bek[0..4].copy_from_slice(&47u32.to_le_bytes());
        bek[12..16].copy_from_slice(&47u32.to_le_bytes());
        assert!(parse_bek_bytes(&bek).is_none());
    }

    #[test]
    fn rejects_metadata_size_copy_mismatch() {
        let mut bek = valid_bek();
        bek[12..16].copy_from_slice(&999u32.to_le_bytes());
        assert!(parse_bek_bytes(&bek).is_none());
    }

    #[test]
    fn rejects_entry_size_underflow_and_overflow() {
        let mut bek = valid_bek();
        bek[48..50].copy_from_slice(&7u16.to_le_bytes());
        assert!(parse_bek_bytes(&bek).is_none());

        let mut bek = valid_bek();
        bek[48..50].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(parse_bek_bytes(&bek).is_none());
    }

    #[test]
    fn rejects_missing_startup_key_entry() {
        let key = metadata_entry(0, 1, &[0u8; 36]);
        let metadata_size = 48u32 + u32::try_from(key.len()).expect("metadata size");
        let mut header = vec![0u8; 48];
        header[0..4].copy_from_slice(&metadata_size.to_le_bytes());
        header[4..8].copy_from_slice(&1u32.to_le_bytes());
        header[8..12].copy_from_slice(&48u32.to_le_bytes());
        header[12..16].copy_from_slice(&metadata_size.to_le_bytes());
        assert!(parse_bek_bytes(&[header, key].concat()).is_none());
    }

    #[test]
    fn rejects_wrong_startup_value_type() {
        let mut external = vec![0u8; 24];
        external.extend_from_slice(&key_property(32));
        let startup = metadata_entry(0x0006, 0x0008, &external);
        let metadata_size = 48u32 + u32::try_from(startup.len()).expect("metadata size");
        let mut header = vec![0u8; 48];
        header[0..4].copy_from_slice(&metadata_size.to_le_bytes());
        header[4..8].copy_from_slice(&1u32.to_le_bytes());
        header[8..12].copy_from_slice(&48u32.to_le_bytes());
        header[12..16].copy_from_slice(&metadata_size.to_le_bytes());
        assert!(parse_bek_bytes(&[header, startup].concat()).is_none());
    }

    #[test]
    fn rejects_truncated_external_key_body() {
        let startup = metadata_entry(0x0006, VALUE_TYPE_EXTERNAL_KEY, &[0u8; 23]);
        let metadata_size = 48u32 + u32::try_from(startup.len()).expect("metadata size");
        let mut header = vec![0u8; 48];
        header[0..4].copy_from_slice(&metadata_size.to_le_bytes());
        header[4..8].copy_from_slice(&1u32.to_le_bytes());
        header[8..12].copy_from_slice(&48u32.to_le_bytes());
        header[12..16].copy_from_slice(&metadata_size.to_le_bytes());
        assert!(parse_bek_bytes(&[header, startup].concat()).is_none());
    }

    #[test]
    fn rejects_missing_nested_key_property() {
        let bek = valid_bek_with_properties(&[description_property("ExternalKey")]);
        assert!(parse_bek_bytes(&bek).is_none());
    }

    #[test]
    fn rejects_duplicate_nested_key_property() {
        let bek = valid_bek_with_properties(&[key_property(32), key_property(32)]);
        assert!(parse_bek_bytes(&bek).is_none());
    }

    #[test]
    fn rejects_duplicate_description_property() {
        let bek = valid_bek_with_properties(&[
            description_property("ExternalKey"),
            description_property("ExternalKey"),
            key_property(32),
        ]);
        assert!(parse_bek_bytes(&bek).is_none());
    }

    #[test]
    fn rejects_duplicate_startup_key_entry() {
        let source = valid_bek();
        let startup = source[48..].to_vec();
        let metadata_size = 48u32 + u32::try_from(startup.len() * 2).expect("metadata size");
        let mut header = vec![0u8; 48];
        header[0..4].copy_from_slice(&metadata_size.to_le_bytes());
        header[4..8].copy_from_slice(&1u32.to_le_bytes());
        header[8..12].copy_from_slice(&48u32.to_le_bytes());
        header[12..16].copy_from_slice(&metadata_size.to_le_bytes());
        assert!(parse_bek_bytes(&[header, startup.clone(), startup].concat()).is_none());
    }

    #[test]
    fn rejects_duplicate_startup_key_entry_with_wrong_value_type() {
        let source = valid_bek();
        let startup = source[48..].to_vec();
        let duplicate = metadata_entry(0x0006, 0x0008, &[0u8; 24]);
        let metadata_size =
            48u32 + u32::try_from(startup.len() + duplicate.len()).expect("metadata size");
        let mut header = vec![0u8; 48];
        header[0..4].copy_from_slice(&metadata_size.to_le_bytes());
        header[4..8].copy_from_slice(&1u32.to_le_bytes());
        header[8..12].copy_from_slice(&48u32.to_le_bytes());
        header[12..16].copy_from_slice(&metadata_size.to_le_bytes());
        assert!(parse_bek_bytes(&[header, startup, duplicate].concat()).is_none());
    }

    #[test]
    fn rejects_top_level_key_property() {
        let source = valid_bek();
        let startup = source[48..].to_vec();
        let top_level_key = key_property(32);
        let metadata_size =
            48u32 + u32::try_from(startup.len() + top_level_key.len()).expect("metadata size");
        let mut header = vec![0u8; 48];
        header[0..4].copy_from_slice(&metadata_size.to_le_bytes());
        header[4..8].copy_from_slice(&1u32.to_le_bytes());
        header[8..12].copy_from_slice(&48u32.to_le_bytes());
        header[12..16].copy_from_slice(&metadata_size.to_le_bytes());
        assert!(parse_bek_bytes(&[header, startup, top_level_key].concat()).is_none());
    }
}
