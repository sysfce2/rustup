use std::{fmt, io::Write};

use termcolor::{Color, ColorSpec, WriteColor};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::{
    format::{self, FormatEvent, FormatFields},
    FmtContext,
};
use tracing_subscriber::registry::LookupSpan;

use crate::utils::notify::NotificationLevel;

macro_rules! debug {
    ( $ ( $ arg : tt ) * ) => ( tracing::trace ! ( $ ( $ arg ) * )  )
}

macro_rules! verbose {
    ( $ ( $ arg : tt ) * ) => ( tracing::debug ! ( $ ( $ arg ) * )  )
}

macro_rules! info {
    ( $ ( $ arg : tt ) * ) => ( tracing::info ! ( $ ( $ arg ) * )  )
}

macro_rules! warn {
    ( $ ( $ arg : tt ) * ) => ( tracing::warn ! ( $ ( $ arg ) * )  )
}

macro_rules! err {
    ( $ ( $ arg : tt ) * ) => ( tracing::error ! ( $ ( $ arg ) * )  )
}

// Adapted from
// https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/trait.FormatEvent.html#examples
pub struct EventFormatter;

impl<S, N> FormatEvent<S, N> for EventFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: format::Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let level = NotificationLevel::from(*event.metadata().level());
        {
            let mut buf = termcolor::Buffer::ansi();
            _ = buf.set_color(ColorSpec::new().set_bold(true).set_fg(level.fg_color()));
            _ = write!(buf, "{level}: ");
            _ = buf.reset();
            writer.write_str(std::str::from_utf8(buf.as_slice()).unwrap())?;
        }
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

impl NotificationLevel {
    fn fg_color(&self) -> Option<Color> {
        match self {
            NotificationLevel::Debug => Some(Color::Blue),
            NotificationLevel::Verbose => Some(Color::Magenta),
            NotificationLevel::Info => None,
            NotificationLevel::Warn => Some(Color::Yellow),
            NotificationLevel::Error => Some(Color::Red),
        }
    }
}
