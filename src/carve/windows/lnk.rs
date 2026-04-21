use crate::carve::windows::WindowsArtefactRecord;
use crate::carve::{
    CarveError, CarveHandler, DeferredWriter, ExtractionContext, PendingCarve, PostCarveMetadata,
    PreValidation, build_carved_file, create_hashers, finalize_hashers, output_path,
};
use crate::evidence::EvidenceSource;
use crate::parsers::time::windows_filetime_to_datetime;
use crate::scanner::NormalizedHit;

const SHELL_LINK_HEADER_SIZE: usize = 0x4C;
const SHELL_LINK_LINK_FLAGS_OFFSET: usize = 20;
const SHELL_LINK_CREATION_TIME_OFFSET: usize = 28;
const SHELL_LINK_ACCESS_TIME_OFFSET: usize = 36;
const SHELL_LINK_WRITE_TIME_OFFSET: usize = 44;
const SHELL_LINK_FILE_SIZE_OFFSET: usize = 52;
const SHELL_LINK_SIGNATURE: [u8; 20] = [
    0x4C, 0x00, 0x00, 0x00, 0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x46,
];

const HAS_LINK_TARGET_ID_LIST: u32 = 0x0000_0001;
const HAS_LINK_INFO: u32 = 0x0000_0002;
const HAS_NAME: u32 = 0x0000_0004;
const HAS_RELATIVE_PATH: u32 = 0x0000_0008;
const HAS_WORKING_DIR: u32 = 0x0000_0010;
const HAS_ARGUMENTS: u32 = 0x0000_0020;
const HAS_ICON_LOCATION: u32 = 0x0000_0040;
const IS_UNICODE: u32 = 0x0000_0080;
const LINK_INFO_FLAG_VOLUME_AND_LOCAL_BASE_PATH: u32 = 0x0000_0001;
const LINK_INFO_FLAG_COMMON_NETWORK_RELATIVE_LINK_AND_PATH_SUFFIX: u32 = 0x0000_0002;

const DEFAULT_LNK_READ_CHUNK: usize = 4096;
const LINK_INFO_HEADER_SIZE: usize = 0x1C;
const LINK_INFO_HEADER_SIZE_WITH_UNICODE: usize = 0x24;
const LINK_INFO_HEADER_SIZE_OFFSET: usize = 4;
const LINK_INFO_FLAGS_OFFSET: usize = 8;
const LINK_INFO_VOLUME_ID_OFFSET: usize = 12;
const LINK_INFO_LOCAL_BASE_PATH_OFFSET: usize = 16;
const LINK_INFO_COMMON_NETWORK_RELATIVE_LINK_OFFSET: usize = 20;
const LINK_INFO_COMMON_PATH_SUFFIX_OFFSET: usize = 24;
const LINK_INFO_LOCAL_BASE_PATH_UNICODE_OFFSET: usize = 28;
const LINK_INFO_COMMON_PATH_SUFFIX_UNICODE_OFFSET: usize = 32;

const COMMON_NETWORK_RELATIVE_LINK_HEADER_SIZE: usize = 0x14;
const COMMON_NETWORK_RELATIVE_LINK_HEADER_SIZE_WITH_UNICODE: usize = 0x24;
const COMMON_NETWORK_RELATIVE_LINK_NET_NAME_OFFSET: usize = 8;
const COMMON_NETWORK_RELATIVE_LINK_NET_NAME_UNICODE_OFFSET: usize = 20;

const VOLUME_ID_MIN_SIZE: usize = 0x10;
const VOLUME_ID_SERIAL_OFFSET: usize = 8;
const EXTRA_DATA_MIN_SIZE: usize = 8;

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

pub struct LnkCarveHandler {
    extension: String,
    min_size: u64,
    max_size: u64,
}

impl LnkCarveHandler {
    pub fn new(extension: String, min_size: u64, max_size: u64) -> Self {
        Self {
            extension,
            min_size,
            max_size,
        }
    }
}

impl CarveHandler for LnkCarveHandler {
    fn file_type(&self) -> &str {
        "lnk"
    }

    fn extension(&self) -> &str {
        &self.extension
    }

    fn is_fast(&self) -> bool {
        true
    }

    fn pre_validate(
        &self,
        evidence: &dyn EvidenceSource,
        offset: u64,
    ) -> Result<PreValidation, CarveError> {
        let mut buf = [0u8; SHELL_LINK_SIGNATURE.len()];
        let n = evidence
            .read_at(offset, &mut buf)
            .map_err(|e| CarveError::Evidence(e.to_string()))?;
        if n < SHELL_LINK_SIGNATURE.len() {
            return Ok(PreValidation::Reject("truncated lnk header".to_string()));
        }
        if buf == SHELL_LINK_SIGNATURE {
            Ok(PreValidation::Proceed)
        } else {
            Ok(PreValidation::Reject("lnk header mismatch".to_string()))
        }
    }

    fn process_hit(
        &self,
        hit: &NormalizedHit,
        ctx: &ExtractionContext,
    ) -> Result<Option<PendingCarve>, CarveError> {
        let max_len = usize::try_from(self.max_size)
            .map_err(|_| CarveError::Invalid("lnk max_size exceeds platform limits".into()))?;
        let (buf, parsed) = match read_and_parse_lnk(ctx, hit.global_offset, max_len) {
            Ok(Some(parsed)) => parsed,
            Ok(None) => return Ok(None),
            Err(err) => return Err(err),
        };

        if parsed.size < self.min_size {
            return Ok(None);
        }

        let size = usize::try_from(parsed.size)
            .map_err(|_| CarveError::Invalid("lnk size exceeds platform limits".into()))?;
        if size > buf.len() {
            return Ok(None);
        }

        let (full_path, rel_path) = output_path(
            ctx.output_root,
            self.file_type(),
            &self.extension,
            hit.global_offset,
        )?;
        let mut writer =
            DeferredWriter::new(full_path, ctx.deferred_buffer_bytes, ctx.metadata_only);
        let (mut md5, mut sha256) = create_hashers(&ctx.hash_config);
        let payload = &buf[..size];
        writer.write_all(payload)?;
        if let Some(ref mut hasher) = md5 {
            hasher.consume(payload);
        }
        if let Some(ref mut hasher) = sha256 {
            use sha2::Digest;
            hasher.update(payload);
        }
        let (md5_hex, sha256_hex) = finalize_hashers(md5, sha256);

        let file = build_carved_file(
            ctx.run_id,
            self.file_type(),
            &self.extension,
            rel_path,
            hit.global_offset,
            parsed.size,
            md5_hex,
            sha256_hex,
            true,
            false,
            Vec::new(),
            &hit.pattern_id,
        );

        Ok(Some(PendingCarve::new(file, writer).with_post_metadata(
            vec![PostCarveMetadata::WindowsArtefact(
                WindowsArtefactRecord::Lnk(parsed.artefact),
            )],
        )))
    }
}

#[derive(Debug)]
struct ParsedLnk {
    size: u64,
    artefact: LnkArtefact,
}

#[derive(Debug, Clone, Copy)]
enum LnkParseError {
    Invalid,
    Truncated,
}

#[derive(Default)]
struct LinkInfoFields {
    volume_serial: Option<String>,
    local_base_path: Option<String>,
    network_path: Option<String>,
    common_path_suffix: Option<String>,
}

struct LnkEvidenceReader<'a> {
    evidence: &'a dyn EvidenceSource,
    offset: u64,
    max_len: usize,
    buf: Vec<u8>,
    chunk: Vec<u8>,
}

impl<'a> LnkEvidenceReader<'a> {
    fn new(evidence: &'a dyn EvidenceSource, offset: u64, max_len: usize) -> Self {
        let chunk_len = max_len.clamp(SHELL_LINK_HEADER_SIZE, DEFAULT_LNK_READ_CHUNK);
        Self {
            evidence,
            offset,
            max_len,
            buf: Vec::with_capacity(chunk_len),
            chunk: vec![0u8; chunk_len],
        }
    }

    fn ensure(&mut self, end: usize) -> Result<(), LnkParseError> {
        if end > self.max_len {
            return Err(LnkParseError::Invalid);
        }
        while self.buf.len() < end {
            let remaining = end - self.buf.len();
            let read_len = remaining.min(self.chunk.len());
            let n = self
                .evidence
                .read_at(
                    self.offset + self.buf.len() as u64,
                    &mut self.chunk[..read_len],
                )
                .map_err(|_| LnkParseError::Invalid)?;
            if n == 0 {
                return Err(LnkParseError::Truncated);
            }
            self.buf.extend_from_slice(&self.chunk[..n]);
        }
        Ok(())
    }

    fn bytes(&self) -> &[u8] {
        &self.buf
    }

    fn into_bytes(mut self, len: usize) -> Vec<u8> {
        self.buf.truncate(len);
        self.buf
    }
}

fn read_and_parse_lnk(
    ctx: &ExtractionContext,
    offset: u64,
    max_len: usize,
) -> Result<Option<(Vec<u8>, ParsedLnk)>, CarveError> {
    let mut reader = LnkEvidenceReader::new(ctx.evidence, offset, max_len);
    match parse_lnk_from_reader(&mut reader, ctx.run_id, offset) {
        Ok(parsed) => {
            let size = usize::try_from(parsed.size)
                .map_err(|_| CarveError::Invalid("lnk size exceeds platform limits".into()))?;
            Ok(Some((reader.into_bytes(size), parsed)))
        }
        Err(LnkParseError::Invalid) | Err(LnkParseError::Truncated) => Ok(None),
    }
}

fn parse_lnk_from_reader(
    reader: &mut LnkEvidenceReader<'_>,
    run_id: &str,
    offset: u64,
) -> Result<ParsedLnk, LnkParseError> {
    reader.ensure(SHELL_LINK_HEADER_SIZE)?;
    let bytes = reader.bytes();
    if bytes[..SHELL_LINK_SIGNATURE.len()] != SHELL_LINK_SIGNATURE {
        return Err(LnkParseError::Invalid);
    }

    let header_size = read_u32(bytes, 0)?;
    if header_size as usize != SHELL_LINK_HEADER_SIZE {
        return Err(LnkParseError::Invalid);
    }

    let link_flags = read_u32(bytes, SHELL_LINK_LINK_FLAGS_OFFSET)?;
    let creation_time =
        windows_filetime_to_datetime(read_u64(bytes, SHELL_LINK_CREATION_TIME_OFFSET)?);
    let access_time = windows_filetime_to_datetime(read_u64(bytes, SHELL_LINK_ACCESS_TIME_OFFSET)?);
    let write_time = windows_filetime_to_datetime(read_u64(bytes, SHELL_LINK_WRITE_TIME_OFFSET)?);
    let file_size = read_u32(bytes, SHELL_LINK_FILE_SIZE_OFFSET)?;

    let mut cursor = SHELL_LINK_HEADER_SIZE;
    if link_flags & HAS_LINK_TARGET_ID_LIST != 0 {
        reader.ensure(cursor.checked_add(2).ok_or(LnkParseError::Invalid)?)?;
        let id_list_size = read_u16(reader.bytes(), cursor)? as usize;
        cursor = cursor
            .checked_add(2)
            .and_then(|value| value.checked_add(id_list_size))
            .ok_or(LnkParseError::Invalid)?;
        reader.ensure(cursor)?;
    }

    let mut link_info = LinkInfoFields::default();
    if link_flags & HAS_LINK_INFO != 0 {
        reader.ensure(cursor.checked_add(4).ok_or(LnkParseError::Invalid)?)?;
        let link_info_size = read_u32(reader.bytes(), cursor)? as usize;
        let link_info_end = cursor
            .checked_add(link_info_size)
            .ok_or(LnkParseError::Invalid)?;
        reader.ensure(link_info_end)?;
        let (parsed, new_cursor) = parse_link_info(reader.bytes(), cursor)?;
        link_info = parsed;
        cursor = new_cursor;
    }

    let is_unicode = link_flags & IS_UNICODE != 0;
    let mut relative_path = None;
    let mut working_dir = None;

    if link_flags & HAS_NAME != 0 {
        cursor = parse_counted_string_from_reader(reader, cursor, is_unicode)?.1;
    }
    if link_flags & HAS_RELATIVE_PATH != 0 {
        let (value, new_cursor) = parse_counted_string_from_reader(reader, cursor, is_unicode)?;
        relative_path = normalize_optional(value);
        cursor = new_cursor;
    }
    if link_flags & HAS_WORKING_DIR != 0 {
        let (value, new_cursor) = parse_counted_string_from_reader(reader, cursor, is_unicode)?;
        working_dir = normalize_optional(value);
        cursor = new_cursor;
    }
    if link_flags & HAS_ARGUMENTS != 0 {
        cursor = parse_counted_string_from_reader(reader, cursor, is_unicode)?.1;
    }
    if link_flags & HAS_ICON_LOCATION != 0 {
        cursor = parse_counted_string_from_reader(reader, cursor, is_unicode)?.1;
    }

    let terminal_end = parse_extra_data_from_reader(reader, cursor)?;
    let local_target = combine_target_parts(
        link_info.local_base_path.as_deref(),
        link_info.common_path_suffix.as_deref(),
    );
    let network_target = combine_target_parts(
        link_info.network_path.as_deref(),
        link_info.common_path_suffix.as_deref(),
    );
    let target_path = local_target
        .clone()
        .or(network_target.clone())
        .or(relative_path);

    Ok(ParsedLnk {
        size: terminal_end as u64,
        artefact: LnkArtefact {
            run_id: run_id.to_string(),
            offset,
            size: terminal_end as u64,
            target_path,
            working_dir,
            creation_time,
            access_time,
            write_time,
            file_size,
            volume_serial: link_info.volume_serial,
            local_base_path: link_info.local_base_path,
            network_path: link_info.network_path,
        },
    })
}

#[cfg(test)]
fn parse_lnk_bytes(bytes: &[u8], run_id: &str, offset: u64) -> Result<ParsedLnk, LnkParseError> {
    if bytes.len() < SHELL_LINK_HEADER_SIZE {
        return Err(LnkParseError::Truncated);
    }
    if bytes[..SHELL_LINK_SIGNATURE.len()] != SHELL_LINK_SIGNATURE {
        return Err(LnkParseError::Invalid);
    }

    let header_size = read_u32(bytes, 0)?;
    if header_size as usize != SHELL_LINK_HEADER_SIZE {
        return Err(LnkParseError::Invalid);
    }

    let link_flags = read_u32(bytes, SHELL_LINK_LINK_FLAGS_OFFSET)?;
    let creation_time =
        windows_filetime_to_datetime(read_u64(bytes, SHELL_LINK_CREATION_TIME_OFFSET)?);
    let access_time = windows_filetime_to_datetime(read_u64(bytes, SHELL_LINK_ACCESS_TIME_OFFSET)?);
    let write_time = windows_filetime_to_datetime(read_u64(bytes, SHELL_LINK_WRITE_TIME_OFFSET)?);
    let file_size = read_u32(bytes, SHELL_LINK_FILE_SIZE_OFFSET)?;

    let mut cursor = SHELL_LINK_HEADER_SIZE;
    if link_flags & HAS_LINK_TARGET_ID_LIST != 0 {
        let id_list_size = read_u16(bytes, cursor)? as usize;
        cursor = cursor
            .checked_add(2 + id_list_size)
            .ok_or(LnkParseError::Invalid)?;
        if cursor > bytes.len() {
            return Err(LnkParseError::Truncated);
        }
    }

    let mut link_info = LinkInfoFields::default();
    if link_flags & HAS_LINK_INFO != 0 {
        let (parsed, new_cursor) = parse_link_info(bytes, cursor)?;
        link_info = parsed;
        cursor = new_cursor;
    }

    let is_unicode = link_flags & IS_UNICODE != 0;
    let mut relative_path = None;
    let mut working_dir = None;

    if link_flags & HAS_NAME != 0 {
        cursor = parse_counted_string(bytes, cursor, is_unicode)?.1;
    }
    if link_flags & HAS_RELATIVE_PATH != 0 {
        let (value, new_cursor) = parse_counted_string(bytes, cursor, is_unicode)?;
        relative_path = normalize_optional(value);
        cursor = new_cursor;
    }
    if link_flags & HAS_WORKING_DIR != 0 {
        let (value, new_cursor) = parse_counted_string(bytes, cursor, is_unicode)?;
        working_dir = normalize_optional(value);
        cursor = new_cursor;
    }
    if link_flags & HAS_ARGUMENTS != 0 {
        cursor = parse_counted_string(bytes, cursor, is_unicode)?.1;
    }
    if link_flags & HAS_ICON_LOCATION != 0 {
        cursor = parse_counted_string(bytes, cursor, is_unicode)?.1;
    }

    let terminal_end = parse_extra_data(bytes, cursor)?;
    let local_target = combine_target_parts(
        link_info.local_base_path.as_deref(),
        link_info.common_path_suffix.as_deref(),
    );
    let network_target = combine_target_parts(
        link_info.network_path.as_deref(),
        link_info.common_path_suffix.as_deref(),
    );
    let target_path = local_target
        .clone()
        .or(network_target.clone())
        .or(relative_path);

    Ok(ParsedLnk {
        size: terminal_end as u64,
        artefact: LnkArtefact {
            run_id: run_id.to_string(),
            offset,
            size: terminal_end as u64,
            target_path,
            working_dir,
            creation_time,
            access_time,
            write_time,
            file_size,
            volume_serial: link_info.volume_serial,
            local_base_path: link_info.local_base_path,
            network_path: link_info.network_path,
        },
    })
}

fn parse_link_info(bytes: &[u8], start: usize) -> Result<(LinkInfoFields, usize), LnkParseError> {
    let size = read_u32(bytes, start)? as usize;
    if size < LINK_INFO_HEADER_SIZE {
        return Err(LnkParseError::Invalid);
    }
    let end = start.checked_add(size).ok_or(LnkParseError::Invalid)?;
    if end > bytes.len() {
        return Err(LnkParseError::Truncated);
    }

    let slice = &bytes[start..end];
    let header_size = read_u32(slice, LINK_INFO_HEADER_SIZE_OFFSET)? as usize;
    if header_size < LINK_INFO_HEADER_SIZE || header_size > slice.len() {
        return Err(LnkParseError::Invalid);
    }
    let link_info_flags = read_u32(slice, LINK_INFO_FLAGS_OFFSET)?;

    let volume_id_offset = read_u32(slice, LINK_INFO_VOLUME_ID_OFFSET)? as usize;
    let local_base_path_offset = read_u32(slice, LINK_INFO_LOCAL_BASE_PATH_OFFSET)? as usize;
    let network_offset = read_u32(slice, LINK_INFO_COMMON_NETWORK_RELATIVE_LINK_OFFSET)? as usize;
    let common_path_suffix_offset = read_u32(slice, LINK_INFO_COMMON_PATH_SUFFIX_OFFSET)? as usize;
    let local_base_path_unicode_offset = if header_size >= LINK_INFO_HEADER_SIZE_WITH_UNICODE {
        read_u32(slice, LINK_INFO_LOCAL_BASE_PATH_UNICODE_OFFSET)? as usize
    } else {
        0
    };
    let common_path_suffix_unicode_offset = if header_size >= LINK_INFO_HEADER_SIZE_WITH_UNICODE {
        read_u32(slice, LINK_INFO_COMMON_PATH_SUFFIX_UNICODE_OFFSET)? as usize
    } else {
        0
    };
    let has_volume = link_info_flags & LINK_INFO_FLAG_VOLUME_AND_LOCAL_BASE_PATH != 0;
    let has_network =
        link_info_flags & LINK_INFO_FLAG_COMMON_NETWORK_RELATIVE_LINK_AND_PATH_SUFFIX != 0;

    if has_volume {
        if volume_id_offset == 0 || local_base_path_offset == 0 {
            return Err(LnkParseError::Invalid);
        }
    } else if volume_id_offset != 0
        || local_base_path_offset != 0
        || local_base_path_unicode_offset != 0
    {
        return Err(LnkParseError::Invalid);
    }
    if has_network {
        if network_offset == 0 {
            return Err(LnkParseError::Invalid);
        }
    } else if network_offset != 0 {
        return Err(LnkParseError::Invalid);
    }

    if volume_id_offset != 0 && volume_id_offset < header_size {
        return Err(LnkParseError::Invalid);
    }
    if network_offset != 0 && network_offset < header_size {
        return Err(LnkParseError::Invalid);
    }

    let volume_end = if volume_id_offset != 0 {
        volume_id_offset
            .checked_add(read_u32(slice, volume_id_offset)? as usize)
            .ok_or(LnkParseError::Invalid)?
    } else {
        header_size
    };
    let network_end = if network_offset != 0 {
        network_offset
            .checked_add(read_u32(slice, network_offset)? as usize)
            .ok_or(LnkParseError::Invalid)?
    } else {
        header_size
    };
    let string_min_offset = header_size.max(volume_end).max(network_end);

    let volume_serial = if volume_id_offset != 0 {
        parse_volume_serial(slice, volume_id_offset)?
    } else {
        None
    };
    let local_base_path = parse_preferred_string(
        slice,
        offset_if_nonzero(local_base_path_unicode_offset),
        offset_if_nonzero(local_base_path_offset),
        string_min_offset,
        string_min_offset,
    )?;
    let network_path = if network_offset != 0 {
        parse_network_path(slice, network_offset)?
    } else {
        None
    };
    let common_path_suffix = parse_preferred_string(
        slice,
        offset_if_nonzero(common_path_suffix_unicode_offset),
        offset_if_nonzero(common_path_suffix_offset),
        string_min_offset,
        string_min_offset,
    )?;

    Ok((
        LinkInfoFields {
            volume_serial,
            local_base_path,
            network_path,
            common_path_suffix,
        },
        end,
    ))
}

fn parse_volume_serial(link_info: &[u8], offset: usize) -> Result<Option<String>, LnkParseError> {
    if offset == 0 {
        return Ok(None);
    }
    let size = read_u32(link_info, offset)? as usize;
    if size < VOLUME_ID_MIN_SIZE {
        return Err(LnkParseError::Invalid);
    }
    let end = offset.checked_add(size).ok_or(LnkParseError::Invalid)?;
    if end > link_info.len() {
        return Err(LnkParseError::Truncated);
    }
    let serial = read_u32(
        link_info,
        offset
            .checked_add(VOLUME_ID_SERIAL_OFFSET)
            .ok_or(LnkParseError::Invalid)?,
    )?;
    Ok(Some(format!(
        "{:04X}-{:04X}",
        (serial >> 16) & 0xFFFF,
        serial & 0xFFFF
    )))
}

fn parse_network_path(link_info: &[u8], offset: usize) -> Result<Option<String>, LnkParseError> {
    let size = read_u32(link_info, offset)? as usize;
    if size < COMMON_NETWORK_RELATIVE_LINK_HEADER_SIZE {
        return Err(LnkParseError::Invalid);
    }
    let end = offset.checked_add(size).ok_or(LnkParseError::Invalid)?;
    if end > link_info.len() {
        return Err(LnkParseError::Truncated);
    }
    let slice = &link_info[offset..end];
    let net_name_offset = read_u32(slice, COMMON_NETWORK_RELATIVE_LINK_NET_NAME_OFFSET)? as usize;
    let has_unicode_header = slice.len() >= COMMON_NETWORK_RELATIVE_LINK_HEADER_SIZE_WITH_UNICODE;
    let ansi_min_offset = if has_unicode_header {
        COMMON_NETWORK_RELATIVE_LINK_HEADER_SIZE_WITH_UNICODE
    } else {
        COMMON_NETWORK_RELATIVE_LINK_HEADER_SIZE
    };
    if net_name_offset != 0 && net_name_offset < ansi_min_offset {
        return Err(LnkParseError::Invalid);
    }
    let net_name_unicode_offset = if has_unicode_header {
        read_u32(slice, COMMON_NETWORK_RELATIVE_LINK_NET_NAME_UNICODE_OFFSET)? as usize
    } else {
        0
    };
    if net_name_unicode_offset != 0
        && net_name_unicode_offset < COMMON_NETWORK_RELATIVE_LINK_HEADER_SIZE_WITH_UNICODE
    {
        return Err(LnkParseError::Invalid);
    }
    parse_preferred_string(
        slice,
        offset_if_nonzero(net_name_unicode_offset),
        offset_if_nonzero(net_name_offset),
        COMMON_NETWORK_RELATIVE_LINK_HEADER_SIZE_WITH_UNICODE,
        ansi_min_offset,
    )
}

fn parse_counted_string_from_reader(
    reader: &mut LnkEvidenceReader<'_>,
    start: usize,
    unicode: bool,
) -> Result<(String, usize), LnkParseError> {
    let count_end = start.checked_add(2).ok_or(LnkParseError::Invalid)?;
    reader.ensure(count_end)?;
    let char_count = read_u16(reader.bytes(), start)? as usize;
    let data_start = count_end;
    let byte_len = if unicode {
        char_count.checked_mul(2).ok_or(LnkParseError::Invalid)?
    } else {
        char_count
    };
    let data_end = data_start
        .checked_add(byte_len)
        .ok_or(LnkParseError::Invalid)?;
    reader.ensure(data_end)?;
    let value = if unicode {
        decode_utf16_string(&reader.bytes()[data_start..data_end])?
    } else {
        decode_ansi_string(&reader.bytes()[data_start..data_end])?
    };
    Ok((value, data_end))
}

#[cfg(test)]
fn parse_counted_string(
    bytes: &[u8],
    start: usize,
    unicode: bool,
) -> Result<(String, usize), LnkParseError> {
    let char_count = read_u16(bytes, start)? as usize;
    let data_start = start.checked_add(2).ok_or(LnkParseError::Invalid)?;
    let byte_len = if unicode {
        char_count.checked_mul(2).ok_or(LnkParseError::Invalid)?
    } else {
        char_count
    };
    let data_end = data_start
        .checked_add(byte_len)
        .ok_or(LnkParseError::Invalid)?;
    if data_end > bytes.len() {
        return Err(LnkParseError::Truncated);
    }

    let value = if unicode {
        decode_utf16_string(&bytes[data_start..data_end])?
    } else {
        decode_ansi_string(&bytes[data_start..data_end])?
    };
    Ok((value, data_end))
}

#[cfg(test)]
fn parse_extra_data(bytes: &[u8], start: usize) -> Result<usize, LnkParseError> {
    let mut cursor = start;
    loop {
        let block_size = read_u32(bytes, cursor)? as usize;
        if block_size == 0 {
            return cursor.checked_add(4).ok_or(LnkParseError::Invalid);
        }
        if block_size < EXTRA_DATA_MIN_SIZE {
            return Err(LnkParseError::Invalid);
        }
        cursor = cursor
            .checked_add(block_size)
            .ok_or(LnkParseError::Invalid)?;
        if cursor > bytes.len() {
            return Err(LnkParseError::Truncated);
        }
    }
}

fn parse_extra_data_from_reader(
    reader: &mut LnkEvidenceReader<'_>,
    start: usize,
) -> Result<usize, LnkParseError> {
    let mut cursor = start;
    loop {
        let header_end = cursor.checked_add(4).ok_or(LnkParseError::Invalid)?;
        reader.ensure(header_end)?;
        let block_size = read_u32(reader.bytes(), cursor)? as usize;
        if block_size == 0 {
            return Ok(header_end);
        }
        if block_size < EXTRA_DATA_MIN_SIZE {
            return Err(LnkParseError::Invalid);
        }
        cursor = cursor
            .checked_add(block_size)
            .ok_or(LnkParseError::Invalid)?;
        reader.ensure(cursor)?;
    }
}

fn parse_preferred_string(
    bytes: &[u8],
    unicode_offset: Option<usize>,
    ansi_offset: Option<usize>,
    unicode_min_offset: usize,
    ansi_min_offset: usize,
) -> Result<Option<String>, LnkParseError> {
    if let Some(unicode_offset) = unicode_offset {
        if unicode_offset < unicode_min_offset {
            return Err(LnkParseError::Invalid);
        }
        let value = decode_null_terminated_utf16(bytes, unicode_offset)?;
        if !value.is_empty() {
            return Ok(Some(value));
        }
    }
    if let Some(ansi_offset) = ansi_offset {
        if ansi_offset < ansi_min_offset {
            return Err(LnkParseError::Invalid);
        }
        let value = decode_null_terminated_ansi(bytes, ansi_offset)?;
        if !value.is_empty() {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn offset_if_nonzero(offset: usize) -> Option<usize> {
    (offset != 0).then_some(offset)
}

fn decode_null_terminated_ansi(bytes: &[u8], offset: usize) -> Result<String, LnkParseError> {
    if offset >= bytes.len() {
        return Err(LnkParseError::Truncated);
    }
    let tail = &bytes[offset..];
    let len = tail
        .iter()
        .position(|&b| b == 0)
        .ok_or(LnkParseError::Truncated)?;
    decode_ansi_string(&tail[..len])
}

fn decode_null_terminated_utf16(bytes: &[u8], offset: usize) -> Result<String, LnkParseError> {
    if offset >= bytes.len() {
        return Err(LnkParseError::Truncated);
    }
    let mut units = Vec::new();
    let mut cursor = offset;
    while cursor + 1 < bytes.len() {
        let unit = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]);
        if unit == 0 {
            return String::from_utf16(&units).map_err(|_| LnkParseError::Invalid);
        }
        units.push(unit);
        cursor += 2;
    }
    Err(LnkParseError::Truncated)
}

fn decode_utf16_string(bytes: &[u8]) -> Result<String, LnkParseError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(LnkParseError::Invalid);
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| LnkParseError::Invalid)
}

fn decode_ansi_string(bytes: &[u8]) -> Result<String, LnkParseError> {
    Ok(bytes.iter().map(|byte| cp1252_decode_byte(*byte)).collect())
}

fn normalize_optional(value: String) -> Option<String> {
    let trimmed = value.trim_matches(char::from(0)).trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn combine_target_parts(base: Option<&str>, suffix: Option<&str>) -> Option<String> {
    match (base, suffix) {
        (Some(base), Some(suffix)) => {
            let base = base.trim_end_matches(char::from(0));
            let suffix = suffix.trim_matches(char::from(0));
            if suffix.is_empty() || ends_with_ignore_ascii_case(base, suffix) {
                Some(base.to_string())
            } else if base.ends_with('\\') || base.ends_with('/') {
                Some(format!("{base}{}", suffix.trim_start_matches(['\\', '/'])))
            } else if suffix.starts_with('\\') || suffix.starts_with('/') {
                Some(format!("{base}{suffix}"))
            } else {
                Some(format!(r"{base}\{suffix}"))
            }
        }
        (Some(base), None) => Some(base.trim_end_matches(char::from(0)).to_string()),
        (None, Some(_)) => None,
        (None, None) => None,
    }
}

fn ends_with_ignore_ascii_case(value: &str, suffix: &str) -> bool {
    let value_bytes = value.as_bytes();
    let suffix_bytes = suffix.as_bytes();
    value_bytes.len() >= suffix_bytes.len()
        && value_bytes[value_bytes.len() - suffix_bytes.len()..].eq_ignore_ascii_case(suffix_bytes)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, LnkParseError> {
    let end = offset.checked_add(2).ok_or(LnkParseError::Truncated)?;
    let slice = bytes.get(offset..end).ok_or(LnkParseError::Truncated)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, LnkParseError> {
    let end = offset.checked_add(4).ok_or(LnkParseError::Truncated)?;
    let slice = bytes.get(offset..end).ok_or(LnkParseError::Truncated)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, LnkParseError> {
    let end = offset.checked_add(8).ok_or(LnkParseError::Truncated)?;
    let slice = bytes.get(offset..end).ok_or(LnkParseError::Truncated)?;
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

fn cp1252_decode_byte(byte: u8) -> char {
    match byte {
        0x80 => '\u{20AC}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8A => '\u{0160}',
        0x8B => '\u{2039}',
        0x8C => '\u{0152}',
        0x8E => '\u{017D}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9A => '\u{0161}',
        0x9B => '\u{203A}',
        0x9C => '\u{0153}',
        0x9E => '\u{017E}',
        0x9F => '\u{0178}',
        _ => char::from(byte),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        COMMON_NETWORK_RELATIVE_LINK_NET_NAME_OFFSET,
        LINK_INFO_COMMON_NETWORK_RELATIVE_LINK_OFFSET, LINK_INFO_FLAGS_OFFSET, LnkCarveHandler,
        SHELL_LINK_HEADER_SIZE, parse_lnk_bytes,
    };
    use crate::carve::{CarveHandler, ExtractionContext};
    use crate::evidence::RawFileSource;
    use crate::scanner::NormalizedHit;
    use std::sync::Arc;

    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/common/lnk_fixture.rs"
    ));

    #[test]
    fn parses_basic_target_path() {
        let bytes = build_lnk(
            Some(r"C:\Windows\System32"),
            None,
            Some("notepad.exe"),
            Some(r"..\..\Windows\System32\notepad.exe"),
            Some(r"C:\Windows"),
        );
        let parsed = parse_lnk_bytes(&bytes, "run_001", 4096).expect("parsed lnk");
        let artefact = parsed.artefact;

        assert_eq!(parsed.size as usize, bytes.len());
        assert_eq!(
            artefact.target_path.as_deref(),
            Some(r"C:\Windows\System32\notepad.exe")
        );
        assert_eq!(artefact.working_dir.as_deref(), Some(r"C:\Windows"));
        assert_eq!(
            artefact.local_base_path.as_deref(),
            Some(r"C:\Windows\System32")
        );
        assert_eq!(artefact.volume_serial.as_deref(), Some("ABCD-1234"));
        assert_eq!(artefact.file_size, 1234);
        assert_eq!(
            artefact.creation_time,
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0)
                .map(|dt| dt.naive_utc())
        );
        assert_eq!(
            artefact.access_time,
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_100, 0)
                .map(|dt| dt.naive_utc())
        );
        assert_eq!(
            artefact.write_time,
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_200, 0)
                .map(|dt| dt.naive_utc())
        );
    }

    #[test]
    fn parses_expected_timestamps() {
        let bytes = build_lnk(
            Some(r"C:\Temp"),
            None,
            Some("report.txt"),
            Some(r".\report.txt"),
            Some(r"C:\Temp"),
        );
        let parsed = parse_lnk_bytes(&bytes, "run_001", 0).expect("parsed lnk");
        let artefact = parsed.artefact;

        assert_eq!(
            artefact.creation_time,
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0)
                .map(|dt| dt.naive_utc())
        );
        assert_eq!(
            artefact.access_time,
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_100, 0)
                .map(|dt| dt.naive_utc())
        );
        assert_eq!(
            artefact.write_time,
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_200, 0)
                .map(|dt| dt.naive_utc())
        );
    }

    #[test]
    fn parses_network_share_path() {
        let bytes = build_lnk_with_options(BuildLnkOptions {
            network_path: Some(r"\\192.168.1.10\share"),
            network_path_unicode: true,
            common_suffix: Some(r"document.docx"),
            relative_path: Some(r"document.docx"),
            ..BuildLnkOptions::default()
        });
        let parsed = parse_lnk_bytes(&bytes, "run_001", 128).expect("parsed lnk");
        let artefact = parsed.artefact;

        assert_eq!(
            artefact.network_path.as_deref(),
            Some(r"\\192.168.1.10\share")
        );
        assert_eq!(
            artefact.target_path.as_deref(),
            Some(r"\\192.168.1.10\share\document.docx")
        );
    }

    #[test]
    fn rejects_truncated_input() {
        let mut bytes = build_lnk(
            Some(r"C:\Temp"),
            None,
            Some("report.txt"),
            Some(r".\report.txt"),
            Some(r"C:\Temp"),
        );
        bytes.truncate(bytes.len() - 2);

        assert!(parse_lnk_bytes(&bytes, "run_001", 0).is_err());
    }

    #[test]
    fn rejects_link_info_offsets_into_header() {
        let mut bytes = build_lnk(
            Some(r"C:\Temp"),
            None,
            Some("report.txt"),
            Some(r".\report.txt"),
            Some(r"C:\Temp"),
        );
        let link_info_start = 0x4C;
        bytes[link_info_start + 16..link_info_start + 20].copy_from_slice(&(0x10u32).to_le_bytes());

        assert!(parse_lnk_bytes(&bytes, "run_001", 0).is_err());
    }

    #[test]
    fn parses_non_ascii_ansi_paths() {
        let bytes = build_lnk_with_options(BuildLnkOptions {
            local_base_path_ansi: Some(b"C:\\Users\\J\xf6rg"),
            common_suffix_ansi: Some(b"Bericht_\x96.txt"),
            working_dir: Some(r"C:\Users"),
            ..BuildLnkOptions::default()
        });
        let parsed = parse_lnk_bytes(&bytes, "run_001", 0).expect("parsed lnk");
        let artefact = parsed.artefact;

        assert_eq!(artefact.local_base_path.as_deref(), Some(r"C:\Users\Jörg"));
        assert_eq!(
            artefact.target_path.as_deref(),
            Some(r"C:\Users\Jörg\Bericht_–.txt")
        );
    }

    #[test]
    fn accepts_unknown_extra_data_blocks() {
        let bytes = build_lnk_with_options(BuildLnkOptions {
            local_base_path: Some(r"C:\Temp"),
            common_suffix: Some("report.txt"),
            relative_path: Some(r".\report.txt"),
            working_dir: Some(r"C:\Temp"),
            extra_data_blocks: &[
                ExtraDataBlock {
                    signature: 0xDEADBEEF,
                    payload: b"abcd",
                },
                ExtraDataBlock {
                    signature: 0xA0000001,
                    payload: b"wxyz",
                },
            ],
            ..BuildLnkOptions::default()
        });
        let parsed = parse_lnk_bytes(&bytes, "run_001", 0).expect("parsed lnk");

        assert_eq!(parsed.size as usize, bytes.len());
    }

    #[test]
    fn rejects_inconsistent_link_info_flags() {
        let mut bytes = build_lnk(
            Some(r"C:\Temp"),
            None,
            Some("report.txt"),
            Some(r".\report.txt"),
            Some(r"C:\Temp"),
        );
        let link_info_start = SHELL_LINK_HEADER_SIZE;
        bytes[link_info_start + LINK_INFO_FLAGS_OFFSET
            ..link_info_start + LINK_INFO_FLAGS_OFFSET + 4]
            .copy_from_slice(&0u32.to_le_bytes());

        assert!(parse_lnk_bytes(&bytes, "run_001", 0).is_err());
    }

    #[test]
    fn rejects_network_offsets_inside_unicode_header() {
        let mut bytes = build_lnk_with_options(BuildLnkOptions {
            network_path: Some(r"\\192.168.1.10\share"),
            network_path_unicode: true,
            common_suffix: Some("document.docx"),
            ..BuildLnkOptions::default()
        });
        let link_info_start = SHELL_LINK_HEADER_SIZE;
        let network_offset = u32::from_le_bytes(
            bytes[link_info_start + LINK_INFO_COMMON_NETWORK_RELATIVE_LINK_OFFSET
                ..link_info_start + LINK_INFO_COMMON_NETWORK_RELATIVE_LINK_OFFSET + 4]
                .try_into()
                .expect("network offset"),
        ) as usize;
        let network_start = link_info_start + network_offset;
        bytes[network_start + COMMON_NETWORK_RELATIVE_LINK_NET_NAME_OFFSET
            ..network_start + COMMON_NETWORK_RELATIVE_LINK_NET_NAME_OFFSET + 4]
            .copy_from_slice(&(0x20u32).to_le_bytes());

        assert!(parse_lnk_bytes(&bytes, "run_001", 0).is_err());
    }

    #[test]
    fn accepts_exact_max_size_boundary() {
        let base_len = build_lnk(None, None, Some(""), None, None).len();
        let suffix = "A".repeat(65_536 - base_len);
        let bytes = build_lnk(None, None, Some(&suffix), None, None);
        assert_eq!(bytes.len(), 65_536);

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let raw_path = temp_dir.path().join("boundary.lnk.raw");
        std::fs::write(&raw_path, &bytes).expect("write raw");
        let evidence = RawFileSource::open(&raw_path).expect("open raw");
        let output_dir = tempfile::tempdir().expect("output");
        let ctx =
            ExtractionContext::new("test_lnk_boundary", output_dir.path(), &evidence, 64 * 1024);
        let hit = NormalizedHit {
            global_offset: 0,
            file_type_id: "lnk".to_string(),
            pattern_id: "lnk_header".to_string(),
            chunk_data: Some(Arc::new(bytes)),
            chunk_start: 0,
        };

        let pending = LnkCarveHandler::new("lnk".to_string(), 76, 65_536)
            .process_hit(&hit, &ctx)
            .expect("process hit");
        assert!(pending.is_some(), "max-size LNK should still carve");
    }
}
