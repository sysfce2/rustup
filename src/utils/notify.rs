use std::fmt;

#[derive(Debug)]
pub(crate) enum NotificationLevel {
    Debug,
    Verbose,
    Info,
    Warn,
    Error,
}

impl fmt::Display for NotificationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NotificationLevel::Debug => "debug",
            NotificationLevel::Verbose => "verbose",
            NotificationLevel::Info => "info",
            NotificationLevel::Warn => "warn",
            NotificationLevel::Error => "error",
        })
    }
}

impl From<tracing::Level> for NotificationLevel {
    fn from(level: tracing::Level) -> Self {
        match level {
            tracing::Level::TRACE => Self::Debug,
            tracing::Level::DEBUG => Self::Verbose,
            tracing::Level::INFO => Self::Info,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::ERROR => Self::Error,
        }
    }
}
