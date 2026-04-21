pub fn push_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

pub fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

pub fn push_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

pub fn filetime_from_unix(secs: i64) -> u64 {
    ((secs + 11_644_473_600) as u64) * 10_000_000
}

pub fn push_counted_string(buf: &mut Vec<u8>, value: &str) {
    let utf16: Vec<u16> = value.encode_utf16().collect();
    push_u16(buf, utf16.len() as u16);
    for unit in utf16 {
        push_u16(buf, unit);
    }
}

pub fn append_null_terminated_ansi_bytes(buf: &mut Vec<u8>, value: &[u8]) -> usize {
    let offset = buf.len();
    buf.extend_from_slice(value);
    buf.push(0);
    offset
}

pub fn append_null_terminated_utf16(buf: &mut Vec<u8>, value: &str) -> usize {
    let offset = buf.len();
    for unit in value.encode_utf16() {
        push_u16(buf, unit);
    }
    push_u16(buf, 0);
    offset
}

pub struct ExtraDataBlock<'a> {
    pub signature: u32,
    pub payload: &'a [u8],
}

#[derive(Default)]
pub struct BuildLnkOptions<'a> {
    pub local_base_path: Option<&'a str>,
    pub local_base_path_ansi: Option<&'a [u8]>,
    pub network_path: Option<&'a str>,
    pub network_path_ansi: Option<&'a [u8]>,
    pub network_path_unicode: bool,
    pub common_suffix: Option<&'a str>,
    pub common_suffix_ansi: Option<&'a [u8]>,
    pub relative_path: Option<&'a str>,
    pub working_dir: Option<&'a str>,
    pub extra_data_blocks: &'a [ExtraDataBlock<'a>],
}

pub fn build_lnk(
    local_base_path: Option<&str>,
    network_path: Option<&str>,
    common_suffix: Option<&str>,
    relative_path: Option<&str>,
    working_dir: Option<&str>,
) -> Vec<u8> {
    build_lnk_with_options(BuildLnkOptions {
        local_base_path,
        network_path,
        common_suffix,
        relative_path,
        working_dir,
        ..BuildLnkOptions::default()
    })
}

pub fn build_lnk_with_options(options: BuildLnkOptions<'_>) -> Vec<u8> {
    let local_base_path_bytes = options
        .local_base_path_ansi
        .map(|bytes| bytes.to_vec())
        .or_else(|| {
            options
                .local_base_path
                .map(|value| value.as_bytes().to_vec())
        });
    let network_path_bytes = options
        .network_path_ansi
        .map(|bytes| bytes.to_vec())
        .or_else(|| options.network_path.map(|value| value.as_bytes().to_vec()));
    let common_suffix_bytes = options
        .common_suffix_ansi
        .map(|bytes| bytes.to_vec())
        .or_else(|| options.common_suffix.map(|value| value.as_bytes().to_vec()))
        .unwrap_or_default();

    let mut buf = Vec::new();
    buf.extend_from_slice(&[
        0x4C, 0x00, 0x00, 0x00, 0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x46,
    ]);
    push_u32(&mut buf, 0x82);
    push_u32(&mut buf, 0x20);
    push_u64(&mut buf, filetime_from_unix(1_700_000_000));
    push_u64(&mut buf, filetime_from_unix(1_700_000_100));
    push_u64(&mut buf, filetime_from_unix(1_700_000_200));
    push_u32(&mut buf, 1234);
    push_u32(&mut buf, 0);
    push_u32(&mut buf, 1);
    push_u16(&mut buf, 0);
    buf.extend_from_slice(&[0u8; 10]);

    let mut link_flags = 0x0000_0082u32;
    if options.relative_path.is_some() {
        link_flags |= 0x0000_0008;
    }
    if options.working_dir.is_some() {
        link_flags |= 0x0000_0010;
    }
    buf[20..24].copy_from_slice(&link_flags.to_le_bytes());

    let mut link_info = vec![0u8; 0x24];
    let mut link_info_flags = 0u32;
    let volume_id_offset = if local_base_path_bytes.is_some() {
        0x24u32
    } else {
        0
    };
    let mut local_base_path_offset = 0u32;
    let mut network_offset = 0u32;
    if let Some(local_base_path_bytes) = local_base_path_bytes {
        link_info_flags |= 0x1;
        link_info.extend_from_slice(&[
            0x10, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x34, 0x12, 0xCD, 0xAB, 0x10, 0x00,
            0x00, 0x00,
        ]);
        local_base_path_offset =
            append_null_terminated_ansi_bytes(&mut link_info, &local_base_path_bytes) as u32;
    }

    if let Some(network_path_bytes) = network_path_bytes {
        link_info_flags |= 0x2;
        network_offset = link_info.len() as u32;
        let mut network = vec![
            0u8;
            if options.network_path_unicode {
                0x24
            } else {
                0x14
            }
        ];
        let net_name_offset = network.len() as u32;
        append_null_terminated_ansi_bytes(&mut network, &network_path_bytes);
        if options.network_path_unicode {
            let net_name_unicode_offset = network.len() as u32;
            network[20..24].copy_from_slice(&net_name_unicode_offset.to_le_bytes());
            append_null_terminated_utf16(
                &mut network,
                options.network_path.expect("unicode network path"),
            );
        }
        let network_size = network.len() as u32;
        network[0..4].copy_from_slice(&network_size.to_le_bytes());
        network[8..12].copy_from_slice(&net_name_offset.to_le_bytes());
        link_info.extend_from_slice(&network);
    }

    let common_suffix_offset =
        append_null_terminated_ansi_bytes(&mut link_info, &common_suffix_bytes) as u32;

    let link_info_size = link_info.len() as u32;
    link_info[0..4].copy_from_slice(&link_info_size.to_le_bytes());
    link_info[4..8].copy_from_slice(&(0x1Cu32).to_le_bytes());
    link_info[8..12].copy_from_slice(&link_info_flags.to_le_bytes());
    link_info[12..16].copy_from_slice(&volume_id_offset.to_le_bytes());
    link_info[16..20].copy_from_slice(&local_base_path_offset.to_le_bytes());
    link_info[20..24].copy_from_slice(&network_offset.to_le_bytes());
    link_info[24..28].copy_from_slice(&common_suffix_offset.to_le_bytes());
    buf.extend_from_slice(&link_info);

    if let Some(relative_path) = options.relative_path {
        push_counted_string(&mut buf, relative_path);
    }
    if let Some(working_dir) = options.working_dir {
        push_counted_string(&mut buf, working_dir);
    }

    for block in options.extra_data_blocks {
        let block_size = 8 + block.payload.len() as u32;
        push_u32(&mut buf, block_size);
        push_u32(&mut buf, block.signature);
        buf.extend_from_slice(block.payload);
    }
    push_u32(&mut buf, 0);
    buf
}
