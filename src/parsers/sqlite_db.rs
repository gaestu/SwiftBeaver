use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, OpenFlags};

use std::collections::HashSet;

use crate::parsers::browser::{
    BrowserCookieRecord,
    BrowserDownloadRecord,
    BrowserHistoryRecord,
};
use crate::parsers::time::{unix_micro_to_datetime, webkit_timestamp_to_datetime};

pub fn extract_browser_history(
    path: &Path,
    run_id: &str,
    source_relative: &str,
) -> Result<Vec<BrowserHistoryRecord>> {
    let mut out = Vec::new();
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    if has_table(&conn, "urls")? {
        if has_table(&conn, "visits")? {
            if let Ok(records) = extract_chrome_visits(&conn, run_id, source_relative) {
                out.extend(records);
            }
        } else if let Ok(records) = extract_chrome_history(&conn, run_id, source_relative) {
            out.extend(records);
        }
    }

    if has_table(&conn, "moz_places")? {
        if has_table(&conn, "moz_historyvisits")? {
            if let Ok(records) = extract_firefox_visits(&conn, run_id, source_relative) {
                out.extend(records);
            }
        } else if let Ok(records) = extract_firefox_history(&conn, run_id, source_relative) {
            out.extend(records);
        }
    }

    Ok(out)
}

pub fn extract_browser_cookies(
    path: &Path,
    run_id: &str,
    source_relative: &str,
) -> Result<Vec<BrowserCookieRecord>> {
    let mut out = Vec::new();
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    if has_table(&conn, "cookies")? {
        if let Ok(records) = extract_chrome_cookies(&conn, run_id, source_relative) {
            out.extend(records);
        }
    }

    if has_table(&conn, "moz_cookies")? {
        if let Ok(records) = extract_firefox_cookies(&conn, run_id, source_relative) {
            out.extend(records);
        }
    }

    Ok(out)
}

pub fn extract_browser_downloads(
    path: &Path,
    run_id: &str,
    source_relative: &str,
) -> Result<Vec<BrowserDownloadRecord>> {
    let mut out = Vec::new();
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    if has_table(&conn, "downloads")? {
        if let Ok(records) = extract_chrome_downloads(&conn, run_id, source_relative) {
            out.extend(records);
        }
    }

    if has_table(&conn, "moz_downloads")? {
        if let Ok(records) = extract_firefox_downloads(&conn, run_id, source_relative) {
            out.extend(records);
        }
    }

    Ok(out)
}

fn has_table(conn: &Connection, name: &str) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name=?1")?;
    let mut rows = stmt.query([name])?;
    Ok(rows.next()?.is_some())
}

fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut out = HashSet::new();
    for row in rows {
        out.insert(row?.to_ascii_lowercase());
    }
    Ok(out)
}

fn pick_col<'a>(columns: &HashSet<String>, candidates: &'a [&'a str]) -> Option<&'a str> {
    for candidate in candidates {
        if columns.contains(&candidate.to_ascii_lowercase()) {
            return Some(*candidate);
        }
    }
    None
}

fn select_col<'a>(columns: &HashSet<String>, candidates: &'a [&'a str], fallback: &'a str) -> &'a str {
    pick_col(columns, candidates).unwrap_or(fallback)
}

fn extract_chrome_history(
    conn: &Connection,
    run_id: &str,
    source_relative: &str,
) -> Result<Vec<BrowserHistoryRecord>> {
    let mut out = Vec::new();
    let columns = table_columns(conn, "urls")?;
    let title_col = select_col(&columns, &["title"], "NULL");
    let visit_col = select_col(&columns, &["last_visit_time"], "NULL");
    let query = format!("SELECT url, {title}, {visit} FROM urls", title = title_col, visit = visit_col);
    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map([], |row| {
        let url: String = row.get(0)?;
        let title: Option<String> = row.get(1)?;
        let last_visit_time: Option<i64> = row.get(2)?;
        Ok((url, title, last_visit_time))
    })?;

    for row in rows {
        let (url, title, last_visit_time) = row?;
        let visit_time = last_visit_time.and_then(webkit_timestamp_to_datetime);
        out.push(BrowserHistoryRecord {
            run_id: run_id.to_string(),
            browser: "chrome".to_string(),
            profile: "Default".to_string(),
            url,
            title,
            visit_time,
            visit_source: None,
            source_file: source_relative.into(),
        });
    }

    Ok(out)
}

fn extract_chrome_visits(
    conn: &Connection,
    run_id: &str,
    source_relative: &str,
) -> Result<Vec<BrowserHistoryRecord>> {
    let mut out = Vec::new();
    let columns = table_columns(conn, "visits")?;
    let visit_col = select_col(&columns, &["visit_time"], "NULL");
    let transition_col = select_col(&columns, &["transition"], "NULL");
    let query = format!(
        "SELECT urls.url, urls.title, visits.{visit}, {transition} FROM visits JOIN urls ON visits.url = urls.id",
        visit = visit_col,
        transition = transition_col,
    );
    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map([], |row| {
        let url: String = row.get(0)?;
        let title: Option<String> = row.get(1)?;
        let visit_time: Option<i64> = row.get(2)?;
        let transition: Option<i64> = row.get(3)?;
        Ok((url, title, visit_time, transition))
    })?;

    for row in rows {
        let (url, title, visit_time, transition) = row?;
        let visit_time = visit_time.and_then(webkit_timestamp_to_datetime);
        let visit_source = transition.map(chrome_transition_label).map(|s| s.to_string());
        out.push(BrowserHistoryRecord {
            run_id: run_id.to_string(),
            browser: "chrome".to_string(),
            profile: "Default".to_string(),
            url,
            title,
            visit_time,
            visit_source,
            source_file: source_relative.into(),
        });
    }

    Ok(out)
}

fn extract_firefox_history(
    conn: &Connection,
    run_id: &str,
    source_relative: &str,
) -> Result<Vec<BrowserHistoryRecord>> {
    let mut out = Vec::new();
    let mut stmt = conn.prepare("SELECT url, title, last_visit_date FROM moz_places")?;
    let rows = stmt.query_map([], |row| {
        let url: String = row.get(0)?;
        let title: Option<String> = row.get(1)?;
        let last_visit_date: Option<i64> = row.get(2)?;
        Ok((url, title, last_visit_date))
    })?;

    for row in rows {
        let (url, title, last_visit_date) = row?;
        let visit_time = last_visit_date.and_then(unix_micro_to_datetime);
        out.push(BrowserHistoryRecord {
            run_id: run_id.to_string(),
            browser: "firefox".to_string(),
            profile: "Default".to_string(),
            url,
            title,
            visit_time,
            visit_source: None,
            source_file: source_relative.into(),
        });
    }

    Ok(out)
}

fn extract_firefox_visits(
    conn: &Connection,
    run_id: &str,
    source_relative: &str,
) -> Result<Vec<BrowserHistoryRecord>> {
    let mut out = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT moz_places.url, moz_places.title, moz_historyvisits.visit_date, moz_historyvisits.visit_type \
         FROM moz_historyvisits JOIN moz_places ON moz_historyvisits.place_id = moz_places.id",
    )?;
    let rows = stmt.query_map([], |row| {
        let url: String = row.get(0)?;
        let title: Option<String> = row.get(1)?;
        let visit_date: Option<i64> = row.get(2)?;
        let visit_type: Option<i64> = row.get(3)?;
        Ok((url, title, visit_date, visit_type))
    })?;

    for row in rows {
        let (url, title, visit_date, visit_type) = row?;
        let visit_time = visit_date.and_then(unix_micro_to_datetime);
        let visit_source = visit_type.map(firefox_visit_label).map(|s| s.to_string());
        out.push(BrowserHistoryRecord {
            run_id: run_id.to_string(),
            browser: "firefox".to_string(),
            profile: "Default".to_string(),
            url,
            title,
            visit_time,
            visit_source,
            source_file: source_relative.into(),
        });
    }

    Ok(out)
}

fn extract_chrome_cookies(
    conn: &Connection,
    run_id: &str,
    source_relative: &str,
) -> Result<Vec<BrowserCookieRecord>> {
    let columns = table_columns(conn, "cookies")?;
    let host_col = match pick_col(&columns, &["host_key", "host"]) {
        Some(col) => col,
        None => return Ok(Vec::new()),
    };
    let name_col = select_col(&columns, &["name"], "NULL");
    let value_col = select_col(&columns, &["value"], "NULL");
    let path_col = select_col(&columns, &["path"], "NULL");
    let expires_col = select_col(&columns, &["expires_utc"], "NULL");
    let last_access_col = select_col(&columns, &["last_access_utc"], "NULL");
    let creation_col = select_col(&columns, &["creation_utc"], "NULL");
    let is_secure_col = select_col(&columns, &["is_secure", "secure"], "NULL");
    let is_http_only_col = select_col(&columns, &["is_httponly", "is_http_only", "httponly"], "NULL");

    let query = format!(
        "SELECT {host}, {name}, {value}, {path}, {expires}, {last_access}, {creation}, {is_secure}, {is_http_only} FROM cookies",
        host = host_col,
        name = name_col,
        value = value_col,
        path = path_col,
        expires = expires_col,
        last_access = last_access_col,
        creation = creation_col,
        is_secure = is_secure_col,
        is_http_only = is_http_only_col,
    );
    let mut stmt = conn.prepare(&query)?;

    let rows = stmt.query_map([], |row| {
        let host: String = row.get(0)?;
        let name: String = row.get(1)?;
        let value: Option<String> = row.get(2)?;
        let path: Option<String> = row.get(3)?;
        let expires_utc: Option<i64> = row.get(4)?;
        let last_access_utc: Option<i64> = row.get(5)?;
        let creation_utc: Option<i64> = row.get(6)?;
        let is_secure: Option<i64> = row.get(7)?;
        let is_http_only: Option<i64> = row.get(8)?;
        Ok((
            host,
            name,
            value,
            path,
            expires_utc,
            last_access_utc,
            creation_utc,
            is_secure,
            is_http_only,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (
            host,
            name,
            value,
            path,
            expires_utc,
            last_access_utc,
            creation_utc,
            is_secure,
            is_http_only,
        ) = row?;
        out.push(BrowserCookieRecord {
            run_id: run_id.to_string(),
            browser: "chrome".to_string(),
            profile: "Default".to_string(),
            host,
            name,
            value,
            path,
            expires_utc: expires_utc.and_then(webkit_timestamp_to_datetime),
            last_access_utc: last_access_utc.and_then(webkit_timestamp_to_datetime),
            creation_utc: creation_utc.and_then(webkit_timestamp_to_datetime),
            is_secure: is_secure.map(|v| v != 0),
            is_http_only: is_http_only.map(|v| v != 0),
            source_file: source_relative.into(),
        });
    }

    Ok(out)
}

fn extract_firefox_cookies(
    conn: &Connection,
    run_id: &str,
    source_relative: &str,
) -> Result<Vec<BrowserCookieRecord>> {
    let mut stmt = conn.prepare(
        "SELECT host, name, value, path, expiry, lastAccessed, creationTime, isSecure, isHttpOnly \
         FROM moz_cookies",
    )?;
    let rows = stmt.query_map([], |row| {
        let host: String = row.get(0)?;
        let name: String = row.get(1)?;
        let value: Option<String> = row.get(2)?;
        let path: Option<String> = row.get(3)?;
        let expiry: Option<i64> = row.get(4)?;
        let last_accessed: Option<i64> = row.get(5)?;
        let creation: Option<i64> = row.get(6)?;
        let is_secure: Option<i64> = row.get(7)?;
        let is_http_only: Option<i64> = row.get(8)?;
        Ok((
            host,
            name,
            value,
            path,
            expiry,
            last_accessed,
            creation,
            is_secure,
            is_http_only,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (host, name, value, path, expiry, last_accessed, creation, is_secure, is_http_only) =
            row?;
        let expires_utc = expiry
            .and_then(|secs| unix_micro_to_datetime(secs.saturating_mul(1_000_000)));
        out.push(BrowserCookieRecord {
            run_id: run_id.to_string(),
            browser: "firefox".to_string(),
            profile: "Default".to_string(),
            host,
            name,
            value,
            path,
            expires_utc,
            last_access_utc: last_accessed.and_then(unix_micro_to_datetime),
            creation_utc: creation.and_then(unix_micro_to_datetime),
            is_secure: is_secure.map(|v| v != 0),
            is_http_only: is_http_only.map(|v| v != 0),
            source_file: source_relative.into(),
        });
    }

    Ok(out)
}

fn extract_chrome_downloads(
    conn: &Connection,
    run_id: &str,
    source_relative: &str,
) -> Result<Vec<BrowserDownloadRecord>> {
    let columns = table_columns(conn, "downloads")?;
    let target_col = match pick_col(&columns, &["target_path", "current_path"]) {
        Some(col) => format!("d.{col}"),
        None => return Ok(Vec::new()),
    };
    let start_col = format!(
        "d.{}",
        select_col(&columns, &["start_time", "startTime", "starttime"], "NULL")
    );
    let end_col = format!(
        "d.{}",
        select_col(&columns, &["end_time", "endTime", "endtime"], "NULL")
    );
    let total_col = format!(
        "d.{}",
        select_col(&columns, &["total_bytes", "totalBytes", "totalbytes"], "NULL")
    );
    let state_col = format!("d.{}", select_col(&columns, &["state"], "NULL"));

    let mut url_candidates: Vec<&str> = Vec::new();
    let mut join_clause = String::new();
    if has_table(conn, "downloads_url_chains")? {
        let chain_cols = table_columns(conn, "downloads_url_chains")?;
        if chain_cols.contains("id") && chain_cols.contains("url") && chain_cols.contains("chain_index") {
            join_clause = " LEFT JOIN downloads_url_chains uc ON d.id = uc.id AND uc.chain_index = 0".to_string();
            url_candidates.push("uc.url");
        }
    }
    for candidate in ["tab_url", "url", "referrer", "site_url", "origin_url"] {
        if columns.contains(candidate) {
            url_candidates.push(match candidate {
                "tab_url" => "d.tab_url",
                "url" => "d.url",
                "referrer" => "d.referrer",
                "site_url" => "d.site_url",
                "origin_url" => "d.origin_url",
                _ => continue,
            });
        }
    }
    let url_expr = if url_candidates.is_empty() {
        "NULL".to_string()
    } else if url_candidates.len() == 1 {
        url_candidates[0].to_string()
    } else {
        format!("COALESCE({})", url_candidates.join(", "))
    };

    let query = format!(
        "SELECT {target}, {url}, {start}, {end}, {total}, {state} FROM downloads d{join}",
        target = target_col,
        url = url_expr,
        start = start_col,
        end = end_col,
        total = total_col,
        state = state_col,
        join = join_clause,
    );
    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map([], |row| {
        let target_path: Option<String> = row.get(0)?;
        let url: Option<String> = row.get(1)?;
        let start_time: Option<i64> = row.get(2)?;
        let end_time: Option<i64> = row.get(3)?;
        let total_bytes: Option<i64> = row.get(4)?;
        let state: Option<i64> = row.get(5)?;
        Ok((target_path, url, start_time, end_time, total_bytes, state))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (target_path, url, start_time, end_time, total_bytes, state) = row?;
        out.push(BrowserDownloadRecord {
            run_id: run_id.to_string(),
            browser: "chrome".to_string(),
            profile: "Default".to_string(),
            url,
            target_path,
            start_time: start_time.and_then(webkit_timestamp_to_datetime),
            end_time: end_time.and_then(webkit_timestamp_to_datetime),
            total_bytes,
            state: state.map(|v| v.to_string()),
            source_file: source_relative.into(),
        });
    }

    Ok(out)
}

fn extract_firefox_downloads(
    conn: &Connection,
    run_id: &str,
    source_relative: &str,
) -> Result<Vec<BrowserDownloadRecord>> {
    let columns = table_columns(conn, "moz_downloads")?;
    let source_col = if columns.contains("source") {
        "source"
    } else if columns.contains("source_uri") {
        "source_uri"
    } else {
        "NULL"
    };
    let target_col = if columns.contains("target") {
        "target"
    } else if columns.contains("target_path") {
        "target_path"
    } else {
        "NULL"
    };
    let start_col = if columns.contains("starttime") {
        "startTime"
    } else if columns.contains("start_time") {
        "start_time"
    } else {
        "NULL"
    };
    let end_col = if columns.contains("endtime") {
        "endTime"
    } else if columns.contains("end_time") {
        "end_time"
    } else {
        "NULL"
    };
    let total_col = if columns.contains("totalbytes") {
        "totalBytes"
    } else if columns.contains("total_bytes") {
        "total_bytes"
    } else {
        "NULL"
    };
    let state_col = if columns.contains("state") { "state" } else { "NULL" };

    let query = format!(
        "SELECT {source}, {target}, {start}, {end}, {total}, {state} FROM moz_downloads",
        source = source_col,
        target = target_col,
        start = start_col,
        end = end_col,
        total = total_col,
        state = state_col,
    );
    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map([], |row| {
        let url: Option<String> = row.get(0)?;
        let target_path: Option<String> = row.get(1)?;
        let start_time: Option<i64> = row.get(2)?;
        let end_time: Option<i64> = row.get(3)?;
        let total_bytes: Option<i64> = row.get(4)?;
        let state: Option<i64> = row.get(5)?;
        Ok((url, target_path, start_time, end_time, total_bytes, state))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (url, target_path, start_time, end_time, total_bytes, state) = row?;
        out.push(BrowserDownloadRecord {
            run_id: run_id.to_string(),
            browser: "firefox".to_string(),
            profile: "Default".to_string(),
            url,
            target_path,
            start_time: start_time.and_then(unix_micro_to_datetime),
            end_time: end_time.and_then(unix_micro_to_datetime),
            total_bytes,
            state: state.map(|v| v.to_string()),
            source_file: source_relative.into(),
        });
    }

    Ok(out)
}

fn chrome_transition_label(transition: i64) -> &'static str {
    match transition & 0xFF {
        0 => "link",
        1 => "typed",
        2 => "auto_bookmark",
        3 => "auto_subframe",
        4 => "manual_subframe",
        5 => "generated",
        6 => "auto_toplevel",
        7 => "form_submit",
        8 => "reload",
        9 => "keyword",
        10 => "keyword_generated",
        _ => "other",
    }
}

fn firefox_visit_label(visit_type: i64) -> &'static str {
    match visit_type {
        1 => "link",
        2 => "typed",
        3 => "bookmark",
        4 => "embed",
        5 => "redirect_permanent",
        6 => "redirect_temporary",
        7 => "download",
        8 => "framed_link",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn extracts_chrome_history() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("History");
        let conn = Connection::open(&path).expect("conn");
        conn.execute(
            "CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT, title TEXT, last_visit_time INTEGER)",
            [],
        )
        .expect("create");
        conn.execute(
            "INSERT INTO urls (url, title, last_visit_time) VALUES (?1, ?2, ?3)",
            ("https://example.com", "Example", 13_303_449_600_000_000i64),
        )
        .expect("insert");
        drop(conn);

        let records = extract_browser_history(&path, "run1", "sqlite/history.sqlite").expect("history");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].browser, "chrome");
        assert_eq!(records[0].url, "https://example.com");
    }

    #[test]
    fn extracts_chrome_visits() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("History");
        let conn = Connection::open(&path).expect("conn");
        conn.execute(
            "CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT, title TEXT)",
            [],
        )
        .expect("create urls");
        conn.execute(
            "CREATE TABLE visits (id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER, transition INTEGER)",
            [],
        )
        .expect("create visits");
        conn.execute(
            "INSERT INTO urls (id, url, title) VALUES (1, ?1, ?2)",
            ("https://example.com", "Example"),
        )
        .expect("insert url");
        conn.execute(
            "INSERT INTO visits (url, visit_time, transition) VALUES (1, ?1, 1)",
            (13_303_449_600_000_000i64,),
        )
        .expect("insert visit");
        drop(conn);

        let records = extract_browser_history(&path, "run1", "sqlite/history.sqlite").expect("history");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].browser, "chrome");
        assert_eq!(records[0].visit_source.as_deref(), Some("typed"));
    }

    #[test]
    fn extracts_firefox_visits() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("places.sqlite");
        let conn = Connection::open(&path).expect("conn");
        conn.execute(
            "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT, title TEXT)",
            [],
        )
        .expect("create places");
        conn.execute(
            "CREATE TABLE moz_historyvisits (id INTEGER PRIMARY KEY, place_id INTEGER, visit_date INTEGER, visit_type INTEGER)",
            [],
        )
        .expect("create visits");
        conn.execute(
            "INSERT INTO moz_places (id, url, title) VALUES (1, ?1, ?2)",
            ("https://example.com", "Example"),
        )
        .expect("insert place");
        conn.execute(
            "INSERT INTO moz_historyvisits (place_id, visit_date, visit_type) VALUES (1, ?1, 2)",
            (1_700_000_000_000_000i64,),
        )
        .expect("insert visit");
        drop(conn);

        let records = extract_browser_history(&path, "run1", "sqlite/history.sqlite").expect("history");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].browser, "firefox");
        assert_eq!(records[0].visit_source.as_deref(), Some("typed"));
    }

    #[test]
    fn extracts_chrome_cookies() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("Cookies");
        let conn = Connection::open(&path).expect("conn");
        conn.execute(
            "CREATE TABLE cookies (host_key TEXT, name TEXT, value TEXT, path TEXT, expires_utc INTEGER, \
             last_access_utc INTEGER, creation_utc INTEGER, is_secure INTEGER, is_httponly INTEGER)",
            [],
        )
        .expect("create cookies");
        conn.execute(
            "INSERT INTO cookies (host_key, name, value, path, expires_utc, last_access_utc, creation_utc, is_secure, is_httponly) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 1)",
            (
                "example.com",
                "sid",
                "abc123",
                "/",
                13_303_449_600_000_000i64,
                13_303_449_600_000_000i64,
                13_303_449_600_000_000i64,
            ),
        )
        .expect("insert cookie");
        drop(conn);

        let records = extract_browser_cookies(&path, "run1", "sqlite/Cookies").expect("cookies");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].browser, "chrome");
        assert_eq!(records[0].host, "example.com");
        assert_eq!(records[0].name, "sid");
    }

    #[test]
    fn extracts_firefox_cookies() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("cookies.sqlite");
        let conn = Connection::open(&path).expect("conn");
        conn.execute(
            "CREATE TABLE moz_cookies (host TEXT, name TEXT, value TEXT, path TEXT, expiry INTEGER, \
             lastAccessed INTEGER, creationTime INTEGER, isSecure INTEGER, isHttpOnly INTEGER)",
            [],
        )
        .expect("create cookies");
        conn.execute(
            "INSERT INTO moz_cookies (host, name, value, path, expiry, lastAccessed, creationTime, isSecure, isHttpOnly) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 1)",
            ("example.com", "sid", "xyz", "/", 1_700_000_000i64, 1_700_000_000_000_000i64, 1_700_000_000_000_000i64),
        )
        .expect("insert cookie");
        drop(conn);

        let records = extract_browser_cookies(&path, "run1", "sqlite/cookies.sqlite").expect("cookies");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].browser, "firefox");
        assert_eq!(records[0].host, "example.com");
        assert_eq!(records[0].name, "sid");
    }

    #[test]
    fn extracts_chrome_downloads() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("History");
        let conn = Connection::open(&path).expect("conn");
        conn.execute(
            "CREATE TABLE downloads (id INTEGER PRIMARY KEY, target_path TEXT, tab_url TEXT, start_time INTEGER, \
             end_time INTEGER, total_bytes INTEGER, state INTEGER)",
            [],
        )
        .expect("create downloads");
        conn.execute(
            "INSERT INTO downloads (target_path, tab_url, start_time, end_time, total_bytes, state) \
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            (
                "/tmp/file.zip",
                "https://example.com/file.zip",
                13_303_449_600_000_000i64,
                13_303_449_600_000_001i64,
                123i64,
            ),
        )
        .expect("insert download");
        drop(conn);

        let records = extract_browser_downloads(&path, "run1", "sqlite/History").expect("downloads");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].browser, "chrome");
        assert_eq!(records[0].url.as_deref(), Some("https://example.com/file.zip"));
    }

    #[test]
    fn extracts_chrome_downloads_from_url_chains() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("History");
        let conn = Connection::open(&path).expect("conn");
        conn.execute(
            "CREATE TABLE downloads (id INTEGER PRIMARY KEY, target_path TEXT, start_time INTEGER, \
             end_time INTEGER, total_bytes INTEGER, state INTEGER)",
            [],
        )
        .expect("create downloads");
        conn.execute(
            "CREATE TABLE downloads_url_chains (id INTEGER, chain_index INTEGER, url TEXT)",
            [],
        )
        .expect("create chains");
        conn.execute(
            "INSERT INTO downloads (id, target_path, start_time, end_time, total_bytes, state) \
             VALUES (1, ?1, ?2, ?3, ?4, 1)",
            (
                "/tmp/file.zip",
                13_303_449_600_000_000i64,
                13_303_449_600_000_001i64,
                123i64,
            ),
        )
        .expect("insert download");
        conn.execute(
            "INSERT INTO downloads_url_chains (id, chain_index, url) VALUES (1, 0, ?1)",
            ("https://edge.example.com/file.zip",),
        )
        .expect("insert chain");
        drop(conn);

        let records = extract_browser_downloads(&path, "run1", "sqlite/History").expect("downloads");
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].url.as_deref(),
            Some("https://edge.example.com/file.zip")
        );
    }

    #[test]
    fn extracts_firefox_downloads() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("downloads.sqlite");
        let conn = Connection::open(&path).expect("conn");
        conn.execute(
            "CREATE TABLE moz_downloads (source TEXT, target TEXT, startTime INTEGER, endTime INTEGER, \
             totalBytes INTEGER, state INTEGER)",
            [],
        )
        .expect("create downloads");
        conn.execute(
            "INSERT INTO moz_downloads (source, target, startTime, endTime, totalBytes, state) \
             VALUES (?1, ?2, ?3, ?4, ?5, 2)",
            (
                "https://example.com/file.zip",
                "/tmp/file.zip",
                1_700_000_000_000_000i64,
                1_700_000_000_000_001i64,
                456i64,
            ),
        )
        .expect("insert download");
        drop(conn);

        let records =
            extract_browser_downloads(&path, "run1", "sqlite/downloads.sqlite").expect("downloads");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].browser, "firefox");
        assert_eq!(records[0].url.as_deref(), Some("https://example.com/file.zip"));
    }
}
