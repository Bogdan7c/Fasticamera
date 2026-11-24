//! Перевод итогов timestamp-запросов (`TimestampCollect`) в длительности проходов для `PassTimeLog`.

use std::time::Duration;

use gpu_vulkan::gpu_timing::TimestampCollect;
use gpu_vulkan::saturation::PassTimeLog;

/// Длительности проходов в порядке `labels`: отрезок принадлежит проходу, которым он кончается (`span.to`).
///
/// `None`, если метки не готовы, не поддерживаются или какого-то прохода нет среди отрезков — тогда кадр
/// считается «без замера», а не нулём.
pub(crate) fn pass_durations(collect: &TimestampCollect, labels: &[&'static str]) -> Option<Vec<Duration>> {
    let TimestampCollect::Ready(spans) = collect else {
        return None;
    };
    labels
        .iter()
        .map(|label| {
            spans
                .spans
                .iter()
                .find(|span| span.to == *label)
                .map(|span| Duration::from_nanos(span.duration_ns))
        })
        .collect()
}

/// Кладёт в журнал замер кадра или отметку «без замера».
pub(crate) fn log_collect(log: &mut PassTimeLog, collect: &TimestampCollect, labels: &[&'static str]) {
    match pass_durations(collect, labels) {
        Some(durations) => log.record_frame(&durations),
        None => log.record_unmeasured_frame(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpu_vulkan::gpu_timing::{TimestampSpan, TimestampSpans};

    fn ready(spans: &[(&'static str, &'static str, u64)]) -> TimestampCollect {
        TimestampCollect::Ready(TimestampSpans {
            spans: spans
                .iter()
                .map(|&(from, to, duration_ns)| TimestampSpan { from, to, duration_ns })
                .collect(),
            total_ns: spans.iter().map(|span| span.2).sum(),
        })
    }

    #[test]
    fn durations_follow_label_order_not_span_order() {
        let collect = ready(&[("start", "a", 10), ("a", "b", 20)]);
        assert_eq!(
            pass_durations(&collect, &["b", "a"]),
            Some(vec![Duration::from_nanos(20), Duration::from_nanos(10)])
        );
    }

    #[test]
    fn missing_pass_or_not_ready_is_unmeasured_not_zero() {
        let collect = ready(&[("start", "a", 10)]);
        assert_eq!(pass_durations(&collect, &["a", "b"]), None);
        assert_eq!(pass_durations(&TimestampCollect::NotReady, &["a"]), None);
        let mut log = PassTimeLog::new(&["a"], 4);
        log_collect(&mut log, &TimestampCollect::Unsupported, &["a"]);
        log_collect(&mut log, &ready(&[("s", "a", 5)]), &["a"]);
        let summary = log.summary();
        assert_eq!((summary.measured_frames, summary.unmeasured_frames), (1, 1));
    }
}
