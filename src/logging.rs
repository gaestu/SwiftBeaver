use tracing_subscriber::EnvFilter;

use crate::cli::LogFormat;

pub fn init_logging() {
    init_logging_with_format(LogFormat::Text);
}

pub fn init_logging_with_format(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    match format {
        LogFormat::Json => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .init();
        }
        LogFormat::Text => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    }
}
