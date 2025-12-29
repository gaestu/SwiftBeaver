pub fn webkit_timestamp_to_datetime(microseconds: i64) -> Option<chrono::NaiveDateTime> {
    if microseconds <= 0 {
        return None;
    }
    let unix_offset_seconds = 11_644_473_600i64;
    let secs = microseconds / 1_000_000 - unix_offset_seconds;
    if secs < 0 {
        return None;
    }
    let nsecs = ((microseconds % 1_000_000).abs() as u32) * 1000;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nsecs).map(|dt| dt.naive_utc())
}

pub fn unix_micro_to_datetime(microseconds: i64) -> Option<chrono::NaiveDateTime> {
    if microseconds <= 0 {
        return None;
    }
    let secs = microseconds / 1_000_000;
    let nsecs = ((microseconds % 1_000_000).abs() as u32) * 1000;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nsecs).map(|dt| dt.naive_utc())
}
