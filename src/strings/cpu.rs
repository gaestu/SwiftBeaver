use crate::chunk::ScanChunk;
use crate::strings::{flags, StringSpan, StringScanner};

pub struct CpuStringScanner {
    min_len: usize,
    max_len: usize,
    scan_utf16: bool,
}

impl CpuStringScanner {
    pub fn new(min_len: usize, max_len: usize, scan_utf16: bool) -> Self {
        let max_len = if max_len == 0 { usize::MAX } else { max_len };
        Self {
            min_len,
            max_len,
            scan_utf16,
        }
    }
}

impl StringScanner for CpuStringScanner {
    fn scan_chunk(&self, chunk: &ScanChunk, data: &[u8]) -> Vec<StringSpan> {
        let mut spans = scan_ascii_runs(data, chunk, self.min_len, self.max_len);
        let mut utf8_spans = scan_utf8_runs(data, chunk, self.min_len, self.max_len);
        spans.append(&mut utf8_spans);

        if self.scan_utf16 {
            let mut utf16_spans = scan_utf16_runs(
                data,
                chunk,
                self.min_len,
                self.max_len,
                true,
            );
            spans.append(&mut utf16_spans);
            let mut utf16_spans = scan_utf16_runs(
                data,
                chunk,
                self.min_len,
                self.max_len,
                false,
            );
            spans.append(&mut utf16_spans);
        }

        spans
    }
}

fn is_printable(byte: u8) -> bool {
    matches!(byte, b'\t' | 0x20..=0x7E)
}

fn scan_ascii_runs(
    data: &[u8],
    chunk: &ScanChunk,
    min_len: usize,
    max_len: usize,
) -> Vec<StringSpan> {
    let mut spans = Vec::new();
    let mut i = 0usize;

    while i < data.len() {
        if !is_printable(data[i]) {
            i += 1;
            continue;
        }

        let start = i;
        let mut len = 0usize;
        while i < data.len() && is_printable(data[i]) {
            i += 1;
            len += 1;
            if len >= max_len {
                break;
            }
        }

        if len >= min_len {
            let slice = &data[start..start + len];
            let span_flags = span_flags_ascii(slice);
            spans.push(StringSpan {
                chunk_id: chunk.id,
                local_start: start as u64,
                length: len as u32,
                flags: span_flags,
            });
        }
    }

    spans
}

pub(crate) fn scan_utf8_runs(
    data: &[u8],
    chunk: &ScanChunk,
    min_len: usize,
    max_len: usize,
) -> Vec<StringSpan> {
    let mut spans = Vec::new();
    let mut i = 0usize;

    while i < data.len() {
        let Some((ch, size)) = decode_utf8_at(data, i) else {
            i += 1;
            continue;
        };
        if !is_printable_unicode(ch) {
            i += size.max(1);
            continue;
        }

        let start = i;
        let mut chars = 0usize;
        let mut end = i;
        let mut has_multibyte = false;
        let mut j = i;

        while j < data.len() && chars < max_len {
            match decode_utf8_at(data, j) {
                Some((ch, size)) if is_printable_unicode(ch) => {
                    if size > 1 {
                        has_multibyte = true;
                    }
                    j += size;
                    chars += 1;
                    end = j;
                }
                _ => break,
            }
        }

        if chars >= min_len && has_multibyte {
            let slice = &data[start..end];
            let mut span_flags = span_flags_ascii(slice);
            span_flags |= flags::UTF8;
            spans.push(StringSpan {
                chunk_id: chunk.id,
                local_start: start as u64,
                length: (end - start) as u32,
                flags: span_flags,
            });
        }

        if j > i {
            i = j;
        } else {
            i += 1;
        }
    }

    spans
}

pub(crate) fn scan_utf16_runs(
    data: &[u8],
    chunk: &ScanChunk,
    min_len: usize,
    max_len: usize,
    little_endian: bool,
) -> Vec<StringSpan> {
    let mut spans = Vec::new();
    let mut start_offset = 0usize;

    while start_offset < 2 {
        let mut i = start_offset;
        while i + 1 < data.len() {
            let (first, second) = (data[i], data[i + 1]);
            let pair_ok = if little_endian {
                is_printable(first) && second == 0
            } else {
                first == 0 && is_printable(second)
            };

            if !pair_ok {
                i += 2;
                continue;
            }

            let run_start = i;
            let mut len = 0usize;
            let mut ascii_bytes = Vec::new();
            let mut j = i;
            while j + 1 < data.len() {
                let (a, b) = (data[j], data[j + 1]);
                let ok = if little_endian {
                    is_printable(a) && b == 0
                } else {
                    a == 0 && is_printable(b)
                };
                if !ok {
                    break;
                }
                let ascii = if little_endian { a } else { b };
                ascii_bytes.push(ascii);
                len += 1;
                if len >= max_len {
                    break;
                }
                j += 2;
            }

            if len >= min_len {
                let mut span_flags = span_flags_ascii(&ascii_bytes);
                span_flags |= if little_endian { flags::UTF16_LE } else { flags::UTF16_BE };
                spans.push(StringSpan {
                    chunk_id: chunk.id,
                    local_start: run_start as u64,
                    length: (len * 2) as u32,
                    flags: span_flags,
                });
            }

            if len >= max_len {
                i = j + 2;
            } else {
                i = j + 2;
            }
        }
        start_offset += 1;
    }

    spans
}

pub(crate) fn span_flags_ascii(slice: &[u8]) -> u32 {
    let mut flags_out = 0u32;
    if contains_case_insensitive(slice, b"http") || contains_case_insensitive(slice, b"www.") {
        flags_out |= flags::URL_LIKE;
    }
    if slice.contains(&b'@') {
        flags_out |= flags::EMAIL_LIKE;
    }
    let digits = slice.iter().filter(|b| b.is_ascii_digit()).count();
    if digits >= 10 {
        flags_out |= flags::PHONE_LIKE;
    }
    flags_out
}

fn contains_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    for window in haystack.windows(needle.len()) {
        let mut matched = true;
        for (b, n) in window.iter().zip(needle.iter()) {
            if b.to_ascii_lowercase() != n.to_ascii_lowercase() {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

fn decode_utf8_at(data: &[u8], idx: usize) -> Option<(char, usize)> {
    let b0 = *data.get(idx)?;
    if b0 < 0x80 {
        return Some((b0 as char, 1));
    }

    let len = data.len();
    if b0 < 0xC2 {
        return None;
    }
    if b0 <= 0xDF {
        if idx + 1 >= len {
            return None;
        }
        let b1 = data[idx + 1];
        if (b1 & 0xC0) != 0x80 {
            return None;
        }
        let code = ((b0 & 0x1F) as u32) << 6 | ((b1 & 0x3F) as u32);
        return std::char::from_u32(code).map(|ch| (ch, 2));
    }
    if b0 <= 0xEF {
        if idx + 2 >= len {
            return None;
        }
        let b1 = data[idx + 1];
        let b2 = data[idx + 2];
        if (b1 & 0xC0) != 0x80 || (b2 & 0xC0) != 0x80 {
            return None;
        }
        if b0 == 0xE0 && b1 < 0xA0 {
            return None;
        }
        if b0 == 0xED && b1 >= 0xA0 {
            return None;
        }
        let code = ((b0 & 0x0F) as u32) << 12
            | ((b1 & 0x3F) as u32) << 6
            | ((b2 & 0x3F) as u32);
        return std::char::from_u32(code).map(|ch| (ch, 3));
    }
    if b0 <= 0xF4 {
        if idx + 3 >= len {
            return None;
        }
        let b1 = data[idx + 1];
        let b2 = data[idx + 2];
        let b3 = data[idx + 3];
        if (b1 & 0xC0) != 0x80 || (b2 & 0xC0) != 0x80 || (b3 & 0xC0) != 0x80 {
            return None;
        }
        if b0 == 0xF0 && b1 < 0x90 {
            return None;
        }
        if b0 == 0xF4 && b1 >= 0x90 {
            return None;
        }
        let code = ((b0 & 0x07) as u32) << 18
            | ((b1 & 0x3F) as u32) << 12
            | ((b2 & 0x3F) as u32) << 6
            | ((b3 & 0x3F) as u32);
        return std::char::from_u32(code).map(|ch| (ch, 4));
    }

    None
}

fn is_printable_unicode(ch: char) -> bool {
    ch == '\t' || !ch.is_control()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_printable_runs() {
        let scanner = CpuStringScanner::new(4, 1024, false);
        let chunk = ScanChunk {
            id: 1,
            start: 0,
            length: 12,
            valid_length: 12,
        };
        let data = b"abc\0defg\nxyz";
        let spans = scanner.scan_chunk(&chunk, data);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].local_start, 4);
        assert_eq!(spans[0].length, 4);
    }

    #[test]
    fn splits_long_strings() {
        let scanner = CpuStringScanner::new(2, 4, false);
        let chunk = ScanChunk {
            id: 1,
            start: 0,
            length: 8,
            valid_length: 8,
        };
        let data = b"abcdefgh";
        let spans = scanner.scan_chunk(&chunk, data);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].length, 4);
        assert_eq!(spans[1].length, 4);
        assert_eq!(spans[1].local_start, 4);
    }

    #[test]
    fn scans_utf16le_runs() {
        let scanner = CpuStringScanner::new(3, 1024, true);
        let chunk = ScanChunk {
            id: 1,
            start: 0,
            length: 16,
            valid_length: 16,
        };
        let data = [
            b'h', 0, b't', 0, b't', 0, b'p', 0, 0, 0, b'x', 0, b'y', 0, b'z', 0,
        ];
        let spans = scanner.scan_chunk(&chunk, &data);
        assert!(spans.iter().any(|span| span.length == 8));
    }

    #[test]
    fn sets_hint_flags_for_ascii() {
        let data = b"see http://example.com mail test@example.com call 4155551234";
        let flags = span_flags_ascii(data);
        assert!((flags & flags::URL_LIKE) != 0);
        assert!((flags & flags::EMAIL_LIKE) != 0);
        assert!((flags & flags::PHONE_LIKE) != 0);
    }

    #[test]
    fn scans_utf8_runs() {
        let chunk = ScanChunk {
            id: 1,
            start: 0,
            length: 16,
            valid_length: 16,
        };
        let data = b"caf\xC3\xA9";
        let spans = scan_utf8_runs(data, &chunk, 4, 1024);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].length, 5);
        assert!((spans[0].flags & flags::UTF8) != 0);
    }
}
