pub const WINDOWS_EPOCH_OFFSET_SECONDS: i64 = 11_644_473_600;

pub fn webkit_timestamp_to_datetime(microseconds: i64) -> Option<chrono::NaiveDateTime> {
    if microseconds <= 0 {
        return None;
    }
    let secs = microseconds / 1_000_000 - WINDOWS_EPOCH_OFFSET_SECONDS;
    if secs < 0 {
        return None;
    }
    let nsecs = ((microseconds % 1_000_000).unsigned_abs() as u32) * 1000;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nsecs).map(|dt| dt.naive_utc())
}

pub fn unix_micro_to_datetime(microseconds: i64) -> Option<chrono::NaiveDateTime> {
    if microseconds <= 0 {
        return None;
    }
    let secs = microseconds / 1_000_000;
    let nsecs = ((microseconds % 1_000_000).unsigned_abs() as u32) * 1000;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nsecs).map(|dt| dt.naive_utc())
}

pub fn windows_filetime_to_datetime(filetime: u64) -> Option<chrono::NaiveDateTime> {
    if filetime == 0 {
        return None;
    }
    let secs = i64::try_from(filetime / 10_000_000).ok()? - WINDOWS_EPOCH_OFFSET_SECONDS;
    let nsecs = ((filetime % 10_000_000) * 100) as u32;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nsecs).map(|dt| dt.naive_utc())
}
