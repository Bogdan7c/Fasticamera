//! Состояние UI-прохода на стороне бэкенда: установленный источник кадра UI и счётчики.
//!
//! Владелец — `VulkanDisplayBackend` (а не сессия показа): источник переживает пересоздание surface, как
//! статистика видоискателя. Объекты GPU рендера UI живут отдельно (`ui_pass::UiPass` в `CameraPipelines`):
//! их нельзя уничтожать, пока в полёте кадры, записанные с ними, поэтому они уходят вместе с пайплайнами
//! цепочки показа, а здесь остаётся только то, что можно держать всегда.
//!
//! Инварианты:
//! * источник вызывается на потоке рендера не чаще раза на показанный кадр и только пока установлен;
//! * любой отказ (источник, рендер) считается и запоминается в `last_error`, но не пробрасывается: UI не может
//!   уронить показ камеры;
//! * счётчики копятся здесь, а не берутся из `UiRenderer::counters`: рендер пересоздаётся вместе с пайплайнами
//!   цепочки, а диагностика обязана пережить это.

use gpu_vulkan::gpu_timing::TimestampCollect;
use ui_draw::draw_list::DrawList;
use ui_draw::frame_source::{UiFrameRequest, UiFrameSource};
use ui_render_vulkan::{PyramidOutcome, UiFrameReport, UiRenderError};

use super::viewfinder_stats::{POINT_UI_DONE, span_ending_at};
use crate::thread::RollingDurations;
use crate::ui_overlay::{UiOverlayCommand, UiOverlayDiagnostics, UiOverlayOutcome};

const LOG_TARGET: &str = "fasticamera::render::ui";

/// Установленный источник и накопленная диагностика UI-прохода.
pub struct UiOverlayState {
    source: Option<Box<dyn UiFrameSource>>,
    installs: u64,
    removals: u64,
    frames_drawn: u64,
    frames_failed: u64,
    source_failures: u64,
    limit_overflows: u64,
    pyramid_rebuilds: u64,
    pyramid_reuses: u64,
    gpu_time: RollingDurations,
    last_error: Option<String>,
}

impl UiOverlayState {
    /// Пустое состояние (UI выключен); окно времени GPU — `gpu_time_window_frames` замеров.
    pub fn new(gpu_time_window_frames: usize) -> Self {
        Self {
            source: None,
            installs: 0,
            removals: 0,
            frames_drawn: 0,
            frames_failed: 0,
            source_failures: 0,
            limit_overflows: 0,
            pyramid_rebuilds: 0,
            pyramid_reuses: 0,
            gpu_time: RollingDurations::new(gpu_time_window_frames),
            last_error: None,
        }
    }

    /// Применяет команду. `Install` заменяет прежний источник (он уничтожается здесь же, на потоке рендера).
    pub fn apply(&mut self, command: UiOverlayCommand) -> UiOverlayOutcome {
        match command {
            UiOverlayCommand::Install(source) => {
                self.source = Some(source);
                self.installs += 1;
                UiOverlayOutcome::Applied
            }
            UiOverlayCommand::Remove => {
                if self.source.take().is_some() {
                    self.removals += 1;
                    UiOverlayOutcome::Applied
                } else {
                    UiOverlayOutcome::AlreadyInEffect
                }
            }
        }
    }

    /// Установлен ли источник (UI-проход включён).
    pub fn is_active(&self) -> bool {
        self.source.is_some()
    }

    /// Просит у источника список рисования. Отказ источника считается (`source_failures`, `last_error`), UI
    /// этого кадра не рисуется — `None`. Без источника тоже `None` (вызывающий проверяет [`Self::is_active`]).
    pub fn produce_draw_list(&mut self, request: &UiFrameRequest) -> Option<DrawList> {
        let source = self.source.as_mut()?;
        match source.produce_frame(request) {
            Ok(list) => Some(list),
            Err(error) => {
                self.source_failures += 1;
                self.last_error = Some(error.to_string());
                tracing::warn!(target: LOG_TARGET, %error, "источник кадра UI отказал: UI этого кадра не рисуется");
                None
            }
        }
    }

    /// Кадр UI записан в команды кадра.
    pub fn note_drawn(&mut self, report: &UiFrameReport) {
        self.frames_drawn += 1;
        match report.pyramid {
            PyramidOutcome::Rebuilt => self.pyramid_rebuilds += 1,
            PyramidOutcome::Reused => self.pyramid_reuses += 1,
            PyramidOutcome::NotNeeded => {}
        }
    }

    /// Рендер UI отказал в записи кадра.
    pub fn note_render_failure(&mut self, error: &UiRenderError) {
        if matches!(error, UiRenderError::LimitExceeded { .. }) {
            self.limit_overflows += 1;
        }
        self.note_frame_skipped(&error.to_string());
    }

    /// Кадр UI пропущен по причине вне источника и записи рендера (например, непригодный масштаб).
    pub fn note_frame_skipped(&mut self, reason: &str) {
        self.frames_failed += 1;
        self.last_error = Some(reason.to_owned());
        tracing::warn!(target: LOG_TARGET, reason, "кадр UI не нарисован: видоискатель показан без UI");
    }

    /// Рендер UI не создался: источник снимается (не ставится), причина — в диагностике. Повторять
    /// попытку на каждом кадре нельзя: сбой создания (память, шейдеры) не лечится сам.
    pub fn reject_source_after_setup_failure(&mut self, error: &UiRenderError) {
        self.source = None;
        self.note_frame_skipped(&format!("рендер UI не создан, источник снят: {error}"));
    }

    /// Метки слота, собранные после его fence: время UI на GPU, если UI в этом кадре рисовался (отрезок
    /// «компоновка готова → UI готов» есть только у таких кадров).
    pub fn note_frame_time(&mut self, collected: &TimestampCollect) {
        if let TimestampCollect::Ready(spans) = collected
            && let Some(ui) = span_ending_at(spans, POINT_UI_DONE)
        {
            self.gpu_time.push(ui);
        }
    }

    /// Снимок для статуса потока.
    pub fn snapshot(&mut self) -> UiOverlayDiagnostics {
        UiOverlayDiagnostics {
            active_source: self.source.as_ref().map(|source| source.name().to_owned()),
            installs: self.installs,
            removals: self.removals,
            frames_drawn: self.frames_drawn,
            frames_failed: self.frames_failed,
            source_failures: self.source_failures,
            limit_overflows: self.limit_overflows,
            pyramid_rebuilds: self.pyramid_rebuilds,
            pyramid_reuses: self.pyramid_reuses,
            gpu_time: self.gpu_time.summary(),
            last_error: self.last_error.clone(),
        }
    }
}

/// Кадр потока рендера для UI-прохода: состояние и ожидаемое время показа этого кадра.
pub struct UiOverlayFrame<'a> {
    /// Источник и счётчики.
    pub state: &'a mut UiOverlayState,
    /// Когда кадр появится на экране (`FrameTiming::expected_present`): к этому моменту строятся анимации.
    pub present_time: shell_contract::clock::MonotonicNanos,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use gpu_vulkan::gpu_timing::{TimestampSpan, TimestampSpans};
    use shell_contract::clock::MonotonicNanos;
    use ui_draw::frame_source::UiFrameError;
    use ui_draw::geometry::PxSize;
    use ui_draw::light::LightDirection;
    use ui_render_vulkan::LimitKind;

    use super::*;
    use crate::vulkan_backend::camera::viewfinder_stats::{POINT_COMPOSE_DONE, POINT_FRAME_START};

    /// Источник: каждый N-й вызов отказывает; считает вызовы и запоминает время показа последнего запроса.
    struct CountingSource {
        calls: Arc<AtomicU64>,
        fail_every: u64,
    }

    impl UiFrameSource for CountingSource {
        fn name(&self) -> &'static str {
            "counting"
        }

        fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_every != 0 && call.is_multiple_of(self.fail_every) {
                return Err(UiFrameError::SourceFailed {
                    reason: format!("отказ на вызове {call}"),
                });
            }
            // Время показа кладём в угол света: тест увидит, что источник получил именно его.
            Ok(DrawList::new(LightDirection::from_angle_degrees(
                request.present_time.as_nanos() as f32,
            )))
        }
    }

    fn request(present_nanos: i64) -> UiFrameRequest {
        UiFrameRequest {
            present_time: MonotonicNanos::from_nanos(present_nanos),
            logical_size: PxSize::new(100.0, 200.0),
            px_per_pt: 2.0,
        }
    }

    fn install(state: &mut UiOverlayState, fail_every: u64) -> Arc<AtomicU64> {
        let calls = Arc::new(AtomicU64::new(0));
        let outcome = state.apply(UiOverlayCommand::Install(Box::new(CountingSource {
            calls: Arc::clone(&calls),
            fail_every,
        })));
        assert_eq!(outcome, UiOverlayOutcome::Applied);
        calls
    }

    #[test]
    fn empty_state_is_off_and_reports_nothing() {
        let mut state = UiOverlayState::new(8);
        assert!(!state.is_active());
        assert_eq!(
            state.produce_draw_list(&request(1)),
            None,
            "без источника списка нет и счётчик не растёт"
        );
        assert_eq!(state.snapshot(), UiOverlayDiagnostics::default());
    }

    #[test]
    fn install_and_remove_report_distinct_outcomes_and_count_only_real_changes() {
        let mut state = UiOverlayState::new(8);
        assert_eq!(
            state.apply(UiOverlayCommand::Remove),
            UiOverlayOutcome::AlreadyInEffect,
            "снимать нечего"
        );
        assert_eq!(state.snapshot().removals, 0, "пустое снятие не считается");
        install(&mut state, 0);
        assert!(state.is_active());
        assert_eq!(state.snapshot().active_source.as_deref(), Some("counting"));
        // Замена: прежний источник уничтожается, `installs` растёт, `removals` — нет.
        install(&mut state, 0);
        let replaced = state.snapshot();
        assert_eq!((replaced.installs, replaced.removals), (2, 0));
        assert_eq!(state.apply(UiOverlayCommand::Remove), UiOverlayOutcome::Applied);
        let removed = state.snapshot();
        assert_eq!((removed.installs, removed.removals), (2, 1));
        assert_eq!(removed.active_source, None);
        assert!(!state.is_active());
    }

    #[test]
    fn source_receives_the_present_time_and_its_failures_are_counted_without_disturbing_the_next_frame() {
        let mut state = UiOverlayState::new(8);
        let calls = install(&mut state, 2);
        let first = state.produce_draw_list(&request(5_000)).expect("первый вызов успешен");
        assert_eq!(
            first.light,
            LightDirection::from_angle_degrees(5_000.0),
            "источнику ушло время показа"
        );
        assert_eq!(state.produce_draw_list(&request(6_000)), None, "второй вызов — отказ");
        let failed = state.snapshot();
        assert_eq!(failed.source_failures, 1);
        assert_eq!(
            failed.last_error.as_deref(),
            Some("источник кадра UI: отказ на вызове 2")
        );
        assert_eq!(failed.frames_failed, 0, "отказ источника — не отказ рендера");
        assert!(
            state.produce_draw_list(&request(7_000)).is_some(),
            "источник остаётся установленным"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn render_outcomes_land_in_their_own_counters() {
        let mut state = UiOverlayState::new(8);
        let report = |pyramid| UiFrameReport {
            pyramid,
            draw_calls: 1,
            shape_instances: 0,
            glyph_instances: 0,
            glass_groups: 1,
        };
        state.note_drawn(&report(PyramidOutcome::Rebuilt));
        state.note_drawn(&report(PyramidOutcome::Reused));
        state.note_drawn(&report(PyramidOutcome::Reused));
        state.note_drawn(&report(PyramidOutcome::NotNeeded));
        state.note_render_failure(&UiRenderError::LimitExceeded {
            kind: LimitKind::GlassGroups,
            requested: 40,
            limit: 32,
        });
        state.note_render_failure(&UiRenderError::SlotInFlight { slot: 1 });
        let snapshot = state.snapshot();
        assert_eq!(
            (
                snapshot.frames_drawn,
                snapshot.pyramid_rebuilds,
                snapshot.pyramid_reuses,
                snapshot.frames_failed,
                snapshot.limit_overflows
            ),
            (4, 1, 2, 2, 1),
            "{snapshot:?}"
        );
        assert!(snapshot.last_error.is_some_and(|error| error.contains("слот кадра 1")));
    }

    #[test]
    fn failed_renderer_setup_drops_the_source_and_keeps_the_reason() {
        let mut state = UiOverlayState::new(8);
        install(&mut state, 0);
        state.reject_source_after_setup_failure(&UiRenderError::SlotOutOfRange { slot: 3, slots: 2 });
        assert!(
            !state.is_active(),
            "источник не остаётся установленным после отказа создания рендера"
        );
        let snapshot = state.snapshot();
        assert_eq!(
            (snapshot.installs, snapshot.removals, snapshot.frames_failed),
            (1, 0, 1)
        );
        assert!(
            snapshot
                .last_error
                .is_some_and(|error| error.contains("рендер UI не создан"))
        );
    }

    #[test]
    fn gpu_time_takes_only_frames_that_drew_ui() {
        let mut state = UiOverlayState::new(8);
        let spans = |points: &[(&'static str, &'static str, u64)]| {
            TimestampCollect::Ready(TimestampSpans {
                spans: points
                    .iter()
                    .map(|(from, to, duration_ns)| TimestampSpan {
                        from,
                        to,
                        duration_ns: *duration_ns,
                    })
                    .collect(),
                total_ns: points.iter().map(|(_, _, duration)| duration).sum(),
            })
        };
        // Кадр без UI: отрезка «ui-done» нет.
        state.note_frame_time(&spans(&[(POINT_FRAME_START, POINT_COMPOSE_DONE, 2_000_000)]));
        assert_eq!(state.snapshot().gpu_time, None);
        // Кадр с UI: берётся именно отрезок «компоновка готова → UI готов».
        state.note_frame_time(&spans(&[
            (POINT_FRAME_START, POINT_COMPOSE_DONE, 2_000_000),
            (POINT_COMPOSE_DONE, POINT_UI_DONE, 700_000),
        ]));
        state.note_frame_time(&TimestampCollect::NotReady);
        let time = state.snapshot().gpu_time.expect("замер есть");
        assert_eq!((time.samples, time.max), (1, Duration::from_micros(700)));
    }
}
