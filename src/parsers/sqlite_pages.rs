use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::Result;

use crate::parsers::browser::BrowserHistoryRecord;
use crate::parsers::time::{unix_micro_to_datetime, webkit_timestamp_to_datetime};
use crate::strings::artifacts::extract_urls_from_text;

const SQLITE_HEADER: &[u8] = b"SQLite format 3\0";
const MAX_TEXT_LEN: usize = 4096;

pub fn extract_history_from_pages(
    path: &Path,
    run_id: &str,
    source_relative: &str,
) -> Result<Vec<BrowserHistoryRecord>> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 100];
    if file.read_exact(&mut header).is_err() {
        return Ok(Vec::new());
    }
    if header.len() < SQLITE_HEADER.len() || &header[..SQLITE_HEADER.len()] != SQLITE_HEADER {
        return Ok(Vec::new());
    }

    let mut page_size = u16::from_be_bytes([header[16], header[17]]) as usize;
    if page_size == 1 {
        page_size = 65_536;
    }
    if page_size < 512 {
        return Ok(Vec::new());
    }
    let reserved = header[20] as usize;
    let usable_size = page_size.saturating_sub(reserved);

    let file_len = file.metadata()?.len() as usize;
    if file_len < SQLITE_HEADER.len() {
        return Ok(Vec::new());
    }

    let page_count = (file_len + page_size - 1) / page_size;
    let mut records: HashMap<String, BrowserHistoryRecord> = HashMap::new();

    for page_index in 0..page_count {
        let offset = page_index * page_size;
        if offset >= file_len {
            break;
        }
        let to_read = std::cmp::min(page_size, file_len - offset);
        let header_offset = if page_index == 0 { 100 } else { 0 };
        if to_read < header_offset + 8 {
            continue;
        }

        let mut page = vec![0u8; to_read];
        if file.seek(SeekFrom::Start(offset as u64)).is_err() {
            continue;
        }
        if file.read_exact(&mut page).is_err() {
            continue;
        }
        if page[header_offset] != 0x0D {
            continue;
        }

        let cell_count = u16::from_be_bytes([page[header_offset + 3], page[header_offset + 4]])
            as usize;
        let cell_ptr_start = header_offset + 8;
        if cell_ptr_start >= page.len() {
            continue;
        }

        for cell_index in 0..cell_count {
            let ptr_offset = cell_ptr_start + cell_index * 2;
            if ptr_offset + 1 >= page.len() {
                break;
            }
            let cell_offset =
                u16::from_be_bytes([page[ptr_offset], page[ptr_offset + 1]]) as usize;
            if cell_offset >= page.len() {
                continue;
            }
            if let Some(payload) = extract_payload(&mut file, &page, cell_offset, page_size, usable_size) {
                let record = parse_record_fields(&payload);
                if record.texts.is_empty() {
                    continue;
                }
                let mut urls = Vec::new();
                for text in &record.texts {
                    urls.extend(extract_urls_from_text(text));
                }
                if urls.is_empty() {
                    continue;
                }
                let title = choose_title(&record.texts, &urls);
                let visit_time = extract_visit_time(&record.ints);
                for url in urls {
                    records
                        .entry(url.clone())
                        .and_modify(|existing| {
                            if existing.title.is_none() {
                                existing.title = title.clone();
                            }
                            if existing.visit_time.is_none() {
                                existing.visit_time = visit_time;
                            }
                        })
                        .or_insert_with(|| BrowserHistoryRecord {
                            run_id: run_id.to_string(),
                            browser: "sqlite_page".to_string(),
                            profile: "unknown".to_string(),
                            url,
                            title: title.clone(),
                            visit_time,
                            visit_source: Some("page_scan".to_string()),
                            source_file: source_relative.into(),
                        });
                }
            }
        }
    }

    Ok(records.into_values().collect())
}

fn extract_payload(
    file: &mut File,
    page: &[u8],
    cell_offset: usize,
    page_size: usize,
    usable_size: usize,
) -> Option<Vec<u8>> {
    let (payload_len, len_size) = read_varint(page.get(cell_offset..)?)?;
    let (_, rowid_size) = read_varint(page.get(cell_offset + len_size..)?)?;
    let payload_start = cell_offset + len_size + rowid_size;
    let payload_len = usize::try_from(payload_len).ok()?;
    let local_len = local_payload_len(payload_len, usable_size);
    let local_end = payload_start.checked_add(local_len)?;
    if local_end > page.len() {
        return None;
    }

    let mut out = Vec::with_capacity(payload_len);
    out.extend_from_slice(&page[payload_start..local_end]);

    if payload_len > local_len {
        let overflow_ptr_offset = local_end;
        if overflow_ptr_offset + 4 > page.len() {
            return None;
        }
        let mut overflow_page = u32::from_be_bytes([
            page[overflow_ptr_offset],
            page[overflow_ptr_offset + 1],
            page[overflow_ptr_offset + 2],
            page[overflow_ptr_offset + 3],
        ]) as u64;
        let mut remaining = payload_len - local_len;
        let overflow_payload = usable_size.saturating_sub(4);
        while overflow_page > 0 && remaining > 0 {
            let offset = overflow_page.saturating_sub(1) * page_size as u64;
            if file.seek(SeekFrom::Start(offset)).is_err() {
                break;
            }
            let mut buf = vec![0u8; page_size];
            if file.read_exact(&mut buf).is_err() {
                break;
            }
            let next_page = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64;
            let take = std::cmp::min(remaining, overflow_payload);
            if 4 + take > buf.len() {
                break;
            }
            out.extend_from_slice(&buf[4..4 + take]);
            remaining -= take;
            overflow_page = next_page;
        }
    }

    Some(out)
}

struct RecordFields {
    texts: Vec<String>,
    ints: Vec<i64>,
}

fn parse_record_fields(payload: &[u8]) -> RecordFields {
    let mut out = RecordFields {
        texts: Vec::new(),
        ints: Vec::new(),
    };
    let Some((header_size, header_len)) = read_varint(payload) else {
        return out;
    };
    let header_size = match usize::try_from(header_size) {
        Ok(size) => size,
        Err(_) => return out,
    };
    if header_size < header_len || header_size > payload.len() {
        return out;
    }

    let mut serials = Vec::new();
    let mut pos = header_len;
    while pos < header_size {
        let Some((serial, consumed)) = read_varint(&payload[pos..]) else {
            return out;
        };
        serials.push(serial);
        pos += consumed;
    }

    let mut data_pos = header_size;
    for serial in serials {
        if data_pos > payload.len() {
            break;
        }
        let (len, is_text) = match serial {
            0 => (0usize, false),
            1 => (1, false),
            2 => (2, false),
            3 => (3, false),
            4 => (4, false),
            5 => (6, false),
            6 => (8, false),
            7 => (8, false),
            8 => (0, false),
            9 => (0, false),
            10 | 11 => (0, false),
            _ => {
                if serial < 12 {
                    (0, false)
                } else if serial % 2 == 0 {
                    let len = (serial - 12) / 2;
                    let len = match usize::try_from(len) {
                        Ok(size) => size,
                        Err(_) => break,
                    };
                    (len, false)
                } else {
                    let len = (serial - 13) / 2;
                    let len = match usize::try_from(len) {
                        Ok(size) => size,
                        Err(_) => break,
                    };
                    (len, true)
                }
            }
        };
        let next_pos = match data_pos.checked_add(len) {
            Some(pos) => pos,
            None => break,
        };
        if next_pos > payload.len() {
            break;
        }
        if matches!(serial, 1 | 2 | 3 | 4 | 5 | 6 | 8 | 9) && len > 0 {
            if let Some(value) = decode_int(&payload[data_pos..next_pos]) {
                out.ints.push(value);
            }
        } else if serial == 8 {
            out.ints.push(0);
        } else if serial == 9 {
            out.ints.push(1);
        }
        if is_text && len > 0 && len <= MAX_TEXT_LEN {
            let text = String::from_utf8_lossy(&payload[data_pos..next_pos]).to_string();
            if !text.trim().is_empty() {
                out.texts.push(text);
            }
        }
        data_pos = next_pos;
    }

    out
}

fn choose_title(texts: &[String], urls: &[String]) -> Option<String> {
    let mut best: Option<&String> = None;
    for text in texts {
        if urls.iter().any(|url| url == text) {
            continue;
        }
        let lower = text.to_ascii_lowercase();
        if lower.contains("http://") || lower.contains("https://") || lower.contains("www.") {
            continue;
        }
        if text.len() > 512 {
            continue;
        }
        if best.map_or(true, |current| text.len() > current.len()) {
            best = Some(text);
        }
    }
    best.cloned()
}

fn extract_visit_time(values: &[i64]) -> Option<chrono::NaiveDateTime> {
    for value in values {
        if let Some(dt) = webkit_timestamp_to_datetime(*value) {
            if is_plausible_time(&dt) {
                return Some(dt);
            }
        }
        if let Some(dt) = unix_micro_to_datetime(*value) {
            if is_plausible_time(&dt) {
                return Some(dt);
            }
        }
    }
    None
}

fn is_plausible_time(dt: &chrono::NaiveDateTime) -> bool {
    let min = chrono::NaiveDateTime::parse_from_str("1990-01-01 00:00:00", "%Y-%m-%d %H:%M:%S")
        .ok();
    let max = chrono::Utc::now().naive_utc() + chrono::Duration::days(2);
    match min {
        Some(min) => *dt >= min && *dt <= max,
        None => *dt <= max,
    }
}

fn decode_int(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() {
        return None;
    }
    let mut value: i128 = 0;
    for &b in bytes {
        value = (value << 8) | i128::from(b);
    }
    let bits = (bytes.len() * 8) as u32;
    let sign_bit = 1i128 << (bits - 1);
    if value & sign_bit != 0 {
        let mask = (1i128 << bits) - 1;
        value = value - mask - 1;
    }
    i64::try_from(value).ok()
}

fn local_payload_len(payload_len: usize, usable_size: usize) -> usize {
    if usable_size <= 32 {
        return payload_len.min(usable_size.saturating_sub(4));
    }
    let max_local = usable_size.saturating_sub(35);
    let min_local = (usable_size.saturating_sub(12) * 32 / 255).saturating_sub(23);
    if payload_len <= max_local {
        payload_len
    } else if usable_size <= 4 {
        payload_len.min(usable_size)
    } else {
        let mut local = min_local + ((payload_len - min_local) % (usable_size - 4));
        if local > max_local {
            local = min_local;
        }
        local
    }
}

fn read_varint(data: &[u8]) -> Option<(u64, usize)> {
    if data.is_empty() {
        return None;
    }
    let mut value = 0u64;
    for i in 0..8 {
        let byte = *data.get(i)?;
        value = (value << 7) | u64::from(byte & 0x7F);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    let byte = *data.get(8)?;
    value = (value << 8) | u64::from(byte);
    Some((value, 9))
}

#[cfg(test)]
mod tests {
    use super::extract_history_from_pages;
    use rusqlite::Connection;
    use tempfile::tempdir;

    #[test]
    fn recovers_urls_from_pages() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("history.sqlite");
        let conn = Connection::open(&path).expect("open");
        conn.execute(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, url TEXT, title TEXT, last_visit_time INTEGER)",
            [],
        )
            .expect("create");
        conn.execute(
            "INSERT INTO t (url, title, last_visit_time) VALUES (?1, ?2, ?3)",
            (
                "https://example.com",
                "Example title",
                13_303_449_600_000_000i64,
            ),
        )
        .expect("insert");
        drop(conn);

        let records = extract_history_from_pages(&path, "run1", "sqlite/history.sqlite")
            .expect("pages");
        let record = records
            .iter()
            .find(|r| r.url == "https://example.com")
            .expect("record");
        assert_eq!(record.title.as_deref(), Some("Example title"));
        assert!(record.visit_time.is_some());
        assert!(records.iter().all(|r| r.browser == "sqlite_page"));
    }

    #[test]
    fn recovers_urls_from_overflow_pages() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("history.sqlite");
        let conn = Connection::open(&path).expect("open");
        conn.execute("PRAGMA page_size=1024", []).expect("pragma");
        conn.execute("VACUUM", []).expect("vacuum");
        conn.execute("CREATE TABLE t (blob TEXT, url TEXT)", [])
            .expect("create");
        let big_text = "A".repeat(8000);
        conn.execute(
            "INSERT INTO t (blob, url) VALUES (?1, ?2)",
            (&big_text, "https://overflow.example.com"),
        )
        .expect("insert");
        drop(conn);

        let records = extract_history_from_pages(&path, "run1", "sqlite/history.sqlite")
            .expect("pages");
        assert!(records
            .iter()
            .any(|r| r.url == "https://overflow.example.com"));
    }
}
