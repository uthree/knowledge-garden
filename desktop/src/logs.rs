//! The app's log: records go to the terminal as usual, and the ones about the local server are
//! also kept in memory for the server page to show.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{EnvFilter, Targets};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

/// How many records the page can scroll back through.
const KEEP: usize = 1000;

#[derive(Debug, Clone)]
pub struct LogRecord {
    /// Counts up from 1 over the app's run; names the record in the view.
    pub seq: u64,
    pub time: chrono::DateTime<chrono::Local>,
    pub level: Level,
    pub target: String,
    pub message: String,
}

#[derive(Default)]
struct Inner {
    records: VecDeque<LogRecord>,
    next: u64,
}

/// The last [`KEEP`] records, shared between the logging layer and the UI.
#[derive(Clone, Default)]
pub struct LogBuffer {
    inner: Arc<Mutex<Inner>>,
    changed: Arc<tokio::sync::Notify>,
}

impl LogBuffer {
    pub fn push(&self, level: Level, target: &str, message: String) {
        {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.next += 1;
            let seq = inner.next;
            if inner.records.len() == KEEP {
                inner.records.pop_front();
            }
            inner.records.push_back(LogRecord {
                seq,
                time: chrono::Local::now(),
                level,
                target: target.to_string(),
                message,
            });
        }
        self.changed.notify_one();
    }

    pub fn records(&self) -> Vec<LogRecord> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.records.iter().cloned().collect()
    }

    pub fn clear(&self) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).records.clear();
        self.changed.notify_one();
    }

    /// Resolves after the next change (at once if one happened since the last call). Works on
    /// any executor.
    pub async fn changed(&self) {
        self.changed.notified().await;
    }
}

/// The buffer the server page reads.
pub fn buffer() -> &'static LogBuffer {
    static BUFFER: OnceLock<LogBuffer> = OnceLock::new();
    BUFFER.get_or_init(LogBuffer::default)
}

/// Sends records to the terminal (filtered by `RUST_LOG`; warnings, and the Garden's own
/// records, by default) and keeps the
/// server's in [`buffer`]: the Garden's own at `info`, one line per request, and database
/// warnings.
pub fn init() {
    let terminal = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn,knowledge_garden=info".into()));
    let kept = Targets::new()
        .with_target("knowledge_garden", Level::INFO)
        .with_target("knowledge_garden::http", Level::DEBUG)
        .with_target("sqlx", Level::WARN);
    let _ = tracing_subscriber::registry()
        .with(terminal)
        .with(
            Capture {
                buffer: buffer().clone(),
            }
            .with_filter(kept),
        )
        .try_init();
}

struct Capture {
    buffer: LogBuffer,
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut text = Text::default();
        event.record(&mut text);
        let meta = event.metadata();
        self.buffer.push(*meta.level(), meta.target(), text.0);
    }
}

/// An event's message followed by its other fields as `name=value`.
#[derive(Default)]
struct Text(String);

impl Visit for Text {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let rest = std::mem::take(&mut self.0);
            let _ = write!(self.0, "{value:?}");
            if !rest.is_empty() {
                self.0.push(' ');
                self.0.push_str(&rest);
            }
        } else {
            if !self.0.is_empty() {
                self.0.push(' ');
            }
            let _ = write!(self.0, "{}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.record_debug(field, &format_args!("{value}"));
        } else {
            self.record_debug(field, &value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_latest_records_with_their_fields() {
        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::registry().with(Capture { buffer: buffer.clone() });
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(peer = "nas", "synced {}", 3);
            for i in 0..KEEP {
                tracing::debug!("line {i}");
            }
        });
        let records = buffer.records();
        assert_eq!(records.len(), KEEP);
        assert_eq!(records.last().unwrap().message, format!("line {}", KEEP - 1));
        assert_eq!(records.last().unwrap().seq, KEEP as u64 + 1);

        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::registry().with(Capture { buffer: buffer.clone() });
        tracing::subscriber::with_default(subscriber, || tracing::warn!(peer = "nas", "synced {}", 3));
        assert_eq!(buffer.records()[0].message, "synced 3 peer=\"nas\"");
        assert_eq!(buffer.records()[0].level, Level::WARN);
    }
}
