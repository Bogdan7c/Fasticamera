//! Сборка [`VsyncTick`] из данных колбэка Choreographer (нейтральная часть, тестируется на ПК).
//!
//! Колбэк `AChoreographer_vsyncCallback` (API 33) получает не одно время, а список «линий кадра»
//! (`AChoreographerFrameCallbackData_getFrameTimelinesLength`): каждая — возможный момент показа
//! со своим сроком сдачи. Платформа сама помечает предпочитаемую линию
//! (`getPreferredFrameTimelineIndex`). Здесь — чистая логика выбора и сборки тика поверх
//! трейта [`FrameTimelineReader`]; FFI-реализация живёт в safety-модуле `choreographer_thread`.

use shell_contract::clock::MonotonicNanos;
use shell_contract::vsync::VsyncTick;

/// Одна линия кадра так, как её отдаёт платформа (всё — `CLOCK_MONOTONIC`, наносекунды).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTimeline {
    pub vsync_id: i64,
    pub expected_present_nanos: i64,
    pub deadline_nanos: i64,
}

/// Доступ к данным одного колбэка Choreographer. Реализация для телефона читает их из
/// `AChoreographerFrameCallbackData`, в тестах — из заготовки.
pub trait FrameTimelineReader {
    /// Время vsync (`getFrameTimeNanos`).
    fn frame_time_nanos(&self) -> i64;
    /// Сколько линий кадра передала платформа.
    fn timeline_count(&self) -> usize;
    /// Индекс линии, которую платформа считает предпочитаемой.
    fn preferred_timeline_index(&self) -> usize;
    /// Линия с индексом `index`; вызывается только с `index < timeline_count()`.
    fn timeline(&self, index: usize) -> FrameTimeline;
}

/// Почему тик не собран. Колбэк логирует и считает такие случаи, но продолжает вызываться.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TimelineError {
    #[error("платформа не передала ни одной линии кадра")]
    NoTimelines,
    #[error("предпочитаемая линия {preferred_index} вне списка из {timeline_count} линий")]
    PreferredIndexOutOfRange {
        preferred_index: usize,
        timeline_count: usize,
    },
}

/// Собирает тик с номером `sequence` из предпочитаемой линии кадра.
///
/// Индекс вне диапазона не «чиним» молча подстановкой другой линии: expected present — основа
/// пейсинга, подмена скрыла бы сбой платформы. Вызывающий пропускает такой кадр и считает ошибку.
pub fn build_tick(reader: &impl FrameTimelineReader, sequence: u64) -> Result<VsyncTick, TimelineError> {
    let timeline_count = reader.timeline_count();
    if timeline_count == 0 {
        return Err(TimelineError::NoTimelines);
    }
    let preferred_index = reader.preferred_timeline_index();
    if preferred_index >= timeline_count {
        return Err(TimelineError::PreferredIndexOutOfRange {
            preferred_index,
            timeline_count,
        });
    }
    let timeline = reader.timeline(preferred_index);
    Ok(VsyncTick {
        sequence,
        frame_time: MonotonicNanos::from_nanos(reader.frame_time_nanos()),
        expected_present: MonotonicNanos::from_nanos(timeline.expected_present_nanos),
        deadline: MonotonicNanos::from_nanos(timeline.deadline_nanos),
        vsync_id: timeline.vsync_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeCallbackData {
        frame_time_nanos: i64,
        preferred_index: usize,
        timelines: Vec<FrameTimeline>,
    }

    impl FrameTimelineReader for FakeCallbackData {
        fn frame_time_nanos(&self) -> i64 {
            self.frame_time_nanos
        }

        fn timeline_count(&self) -> usize {
            self.timelines.len()
        }

        fn preferred_timeline_index(&self) -> usize {
            self.preferred_index
        }

        fn timeline(&self, index: usize) -> FrameTimeline {
            self.timelines[index]
        }
    }

    fn timeline(vsync_id: i64, expected: i64, deadline: i64) -> FrameTimeline {
        FrameTimeline {
            vsync_id,
            expected_present_nanos: expected,
            deadline_nanos: deadline,
        }
    }

    #[test]
    fn tick_takes_values_from_the_preferred_timeline_not_the_first() {
        let data = FakeCallbackData {
            frame_time_nanos: 1_000,
            preferred_index: 1,
            timelines: vec![
                timeline(10, 2_000, 1_500),
                timeline(11, 3_000, 2_500),
                timeline(12, 4_000, 3_500),
            ],
        };
        let tick = build_tick(&data, 42).expect("линия есть");
        assert_eq!(42, tick.sequence);
        assert_eq!(1_000, tick.frame_time.as_nanos());
        assert_eq!(3_000, tick.expected_present.as_nanos());
        assert_eq!(2_500, tick.deadline.as_nanos());
        assert_eq!(11, tick.vsync_id);
    }

    #[test]
    fn empty_timeline_list_is_an_error_not_a_default_tick() {
        let data = FakeCallbackData {
            frame_time_nanos: 1,
            preferred_index: 0,
            timelines: Vec::new(),
        };
        assert_eq!(Err(TimelineError::NoTimelines), build_tick(&data, 1));
    }

    #[test]
    fn preferred_index_past_the_end_is_reported_with_both_numbers() {
        let data = FakeCallbackData {
            frame_time_nanos: 1,
            preferred_index: 3,
            timelines: vec![timeline(1, 2, 3), timeline(4, 5, 6)],
        };
        assert_eq!(
            Err(TimelineError::PreferredIndexOutOfRange {
                preferred_index: 3,
                timeline_count: 2
            }),
            build_tick(&data, 1)
        );
    }

    #[test]
    fn last_valid_index_is_accepted() {
        let data = FakeCallbackData {
            frame_time_nanos: 7,
            preferred_index: 1,
            timelines: vec![timeline(1, 2, 3), timeline(4, 5, 6)],
        };
        assert_eq!(4, build_tick(&data, 9).expect("индекс в диапазоне").vsync_id);
    }
}
