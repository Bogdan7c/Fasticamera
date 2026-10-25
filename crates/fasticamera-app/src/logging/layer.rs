//! Слой `tracing-subscriber`: событие → `LogRecord` → приёмник.

use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use super::record::LogRecord;
use super::sink::LogSink;

/// Слой, который отдаёт каждое прошедшее фильтр событие приёмнику `Sink`.
/// Спаны пока не печатаются: в каркасе их нет, а формат для них выберем, когда появятся.
pub struct SinkLayer<Sink: LogSink> {
    sink: Sink,
}

impl<Sink: LogSink> SinkLayer<Sink> {
    pub fn new(sink: Sink) -> Self {
        Self { sink }
    }
}

impl<S: Subscriber, Sink: LogSink> Layer<S> for SinkLayer<Sink> {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        self.sink.write(&LogRecord::from_event(event));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::logging::build_subscriber;
    use crate::logging::record::LogPriority;
    use crate::runtime_config::LogFilter;

    /// Тестовый приёмник: копит записи, чтобы проверить итог конвейера.
    #[derive(Clone, Default)]
    struct CollectingSink {
        records: Arc<Mutex<Vec<LogRecord>>>,
    }

    impl LogSink for CollectingSink {
        fn write(&self, record: &LogRecord) {
            self.records.lock().expect("mutex").push(record.clone());
        }
    }

    fn collect_with(filter: &str, emit: impl FnOnce()) -> Vec<LogRecord> {
        let sink = CollectingSink::default();
        let subscriber = build_subscriber(&LogFilter::parse(filter).expect("фильтр"), sink.clone());
        tracing::subscriber::with_default(subscriber, emit);
        sink.records.lock().expect("mutex").clone()
    }

    #[test]
    fn event_reaches_sink_with_target_message_and_fields() {
        let records = collect_with("info", || {
            tracing::info!(target: "fasticamera::test", frames = 60, "поток запущен {}", "ok");
        });
        assert_eq!(
            records,
            vec![LogRecord::new(
                LogPriority::Info,
                "fasticamera::test: поток запущен ok frames=60".to_owned()
            )]
        );
    }

    #[test]
    fn filter_drops_events_below_default_level() {
        let records = collect_with("warn", || {
            tracing::info!(target: "fasticamera::test", "не должно пройти");
            tracing::warn!(target: "fasticamera::test", "должно пройти");
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].priority, LogPriority::Warn);
    }

    #[test]
    fn per_target_directive_overrides_default_level() {
        let records = collect_with("warn,fasticamera::camera=trace", || {
            tracing::trace!(target: "fasticamera::camera", "подробно");
            tracing::debug!(target: "fasticamera::render", "отброшено");
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].priority, LogPriority::Verbose);
        assert!(records[0].text.starts_with("fasticamera::camera: подробно"));
    }

    #[test]
    fn string_fields_are_quoted() {
        let records = collect_with("info", || {
            tracing::info!(target: "t", scenario = "hello", "старт");
        });
        assert_eq!(records[0].text, "t: старт scenario=\"hello\"");
    }
}
