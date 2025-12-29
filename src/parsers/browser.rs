use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BrowserHistoryRecord {
    pub run_id: String,
    pub browser: String,
    pub profile: String,
    pub url: String,
    pub title: Option<String>,
    pub visit_time: Option<chrono::NaiveDateTime>,
    pub visit_source: Option<String>,
    pub source_file: std::path::PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowserCookieRecord {
    pub run_id: String,
    pub browser: String,
    pub profile: String,
    pub host: String,
    pub name: String,
    pub value: Option<String>,
    pub path: Option<String>,
    pub expires_utc: Option<chrono::NaiveDateTime>,
    pub last_access_utc: Option<chrono::NaiveDateTime>,
    pub creation_utc: Option<chrono::NaiveDateTime>,
    pub is_secure: Option<bool>,
    pub is_http_only: Option<bool>,
    pub source_file: std::path::PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowserDownloadRecord {
    pub run_id: String,
    pub browser: String,
    pub profile: String,
    pub url: Option<String>,
    pub target_path: Option<String>,
    pub start_time: Option<chrono::NaiveDateTime>,
    pub end_time: Option<chrono::NaiveDateTime>,
    pub total_bytes: Option<i64>,
    pub state: Option<String>,
    pub source_file: std::path::PathBuf,
}
