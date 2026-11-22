//! Выбор кадра ответчика рендера для тестов сценария `capture` в маршрутизаторе (вынесено ради предела размера файла).

/// Выбор кадра ответчика рендера. Метка сенсора строится от НАСТОЯЩИХ часов процесса и момента нажатия плана, а не
/// константой: тогда сценарий, переводящий её через измеренное смещение часов, получает ту задержку, что задана здесь.
#[derive(Clone, Copy)]
pub(super) struct ResponderChoice {
    /// Сколько мс назад (до нажатия) началась экспозиция выбранного кадра.
    pub(super) latency_ms: i64,
    /// `None` — ровно показанный кадр; `Some(причина)` — замена.
    pub(super) substituted: Option<still_contract::frame_choice::SubstitutionReason>,
}

impl Default for ResponderChoice {
    fn default() -> Self {
        Self {
            latency_ms: 80,
            substituted: None,
        }
    }
}

impl ResponderChoice {
    /// Выбор для плана: метка сенсора = нажатие в шкале BOOTTIME − задержка.
    pub(super) fn for_press(
        self,
        pressed_at: shell_contract::clock::MonotonicNanos,
    ) -> still_contract::frame_choice::FrameChoice {
        use crate::clock_offset::{ClockOffsetSource, SystemClocks};
        let offset = SystemClocks.sample_offset().expect("системные часы читаются");
        let sensor_timestamp = camera_contract::capture_result::SensorTimestamp::from_nanos(
            offset.monotonic_to_boottime(pressed_at).as_nanos() - self.latency_ms * 1_000_000,
        );
        match self.substituted {
            None => still_contract::frame_choice::FrameChoice::ShownAtPress {
                sensor_timestamp,
                shown_at: shell_contract::clock::MonotonicNanos::from_nanos(pressed_at.as_nanos() - 10_000_000),
            },
            Some(reason) => still_contract::frame_choice::FrameChoice::Substituted {
                sensor_timestamp,
                wanted: None,
                reason,
            },
        }
    }
}
