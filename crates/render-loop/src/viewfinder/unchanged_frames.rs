//! Счёт решений «рисовать ли этот тик» (решение 214, сессия 16a): сколько тиков пропущено потому, что картинка
//! экрана не изменилась, и по какой причине остальные тики всё же нарисованы.
//!
//! Нужен двум читателям: тестам (каждое условие, запрещающее пропуск, видно отдельным счётчиком) и сценариям
//! на телефоне (по доле пропусков видно, работает ли пропуск на 120 Гц, а по причинам — что его держит).
//! Типы нейтральные (без Vulkan): их строит бэкенд показа и читают поток рендера и сценарии.

/// Почему тик нарисован, а не пропущен. Каждая причина — отдельное условие, запрещающее пропуск; порядок проверки
/// в решении (`session::unchanged`) совпадает с порядком вариантов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawReason {
    /// Политика конфига — рисовать каждый тик ([`crate::config::UnchangedFramePolicy::AlwaysRedraw`]).
    PolicyAlwaysRedraw,
    /// Замер A/B подавляет работу GPU: кадры должны рисоваться как при обычной работе, иначе замер не сравним.
    GpuWorkSuppressed,
    /// В тике есть новый кадр камеры (или отладочная мозаика): картинка ISP изменилась.
    NewCameraFrame,
    /// Тик без нового кадра, но экран не «тихо живой»: удержание, чёрный, переход, возврат кадра камере и т. п.
    NotQuietLiveRecompose,
    /// Источник UI есть, но кадр UI не получен (отказ источника, рендера, размера): состояние UI на экране
    /// неизвестно, сравнивать нечего.
    UiUnavailable,
    /// Показанного кадра для сравнения нет: первый кадр, после смены поверхности, ошибки записи или показа.
    NoBaseline,
    /// Изменилась настройка видоискателя (команда): вид, место, кадрирование, приборы, подавление.
    SceneChanged,
    /// Список рисования UI отличается от показанного.
    UiChanged,
    /// Подряд пропущено предельное число тиков (`max_consecutive_skips`): нужен слот кольца для итогов кадров.
    SkipRunLimit,
}

/// Счётчики решений (накопительные за жизнь бэкенда, переживают пересоздание surface).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct UnchangedFrameCounters {
    /// Сколько тиков пропущено: ни записи, ни показа, на экране прежний кадр.
    pub skipped: u64,
    /// Нарисовано из-за политики `AlwaysRedraw`.
    pub drawn_policy_always_redraw: u64,
    /// Нарисовано из-за подавления работы GPU замером A/B.
    pub drawn_gpu_work_suppressed: u64,
    /// Нарисовано из-за нового кадра камеры.
    pub drawn_new_camera_frame: u64,
    /// Нарисовано, потому что тик без нового кадра не был тихой живой перекомпоновкой.
    pub drawn_not_quiet_live_recompose: u64,
    /// Нарисовано, потому что кадр UI не получен.
    pub drawn_ui_unavailable: u64,
    /// Нарисовано, потому что сравнивать не с чем.
    pub drawn_no_baseline: u64,
    /// Нарисовано из-за смены настроек видоискателя.
    pub drawn_scene_changed: u64,
    /// Нарисовано из-за изменившегося UI.
    pub drawn_ui_changed: u64,
    /// Нарисовано из-за предела серии пропусков.
    pub drawn_skip_run_limit: u64,
}

impl UnchangedFrameCounters {
    /// Тик пропущен.
    pub fn note_skipped(&mut self) {
        self.skipped += 1;
    }

    /// Тик нарисован по причине `reason`.
    pub fn note_drawn(&mut self, reason: DrawReason) {
        let counter = match reason {
            DrawReason::PolicyAlwaysRedraw => &mut self.drawn_policy_always_redraw,
            DrawReason::GpuWorkSuppressed => &mut self.drawn_gpu_work_suppressed,
            DrawReason::NewCameraFrame => &mut self.drawn_new_camera_frame,
            DrawReason::NotQuietLiveRecompose => &mut self.drawn_not_quiet_live_recompose,
            DrawReason::UiUnavailable => &mut self.drawn_ui_unavailable,
            DrawReason::NoBaseline => &mut self.drawn_no_baseline,
            DrawReason::SceneChanged => &mut self.drawn_scene_changed,
            DrawReason::UiChanged => &mut self.drawn_ui_changed,
            DrawReason::SkipRunLimit => &mut self.drawn_skip_run_limit,
        };
        *counter += 1;
    }

    /// Сколько тиков нарисовано по всем причинам вместе.
    pub fn drawn_total(&self) -> u64 {
        self.drawn_policy_always_redraw
            + self.drawn_gpu_work_suppressed
            + self.drawn_new_camera_frame
            + self.drawn_not_quiet_live_recompose
            + self.drawn_ui_unavailable
            + self.drawn_no_baseline
            + self.drawn_scene_changed
            + self.drawn_ui_changed
            + self.drawn_skip_run_limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Все причины, по порядку объявления: тест ниже проверяет, что каждая попадает в свой счётчик.
    const ALL_REASONS: [DrawReason; 9] = [
        DrawReason::PolicyAlwaysRedraw,
        DrawReason::GpuWorkSuppressed,
        DrawReason::NewCameraFrame,
        DrawReason::NotQuietLiveRecompose,
        DrawReason::UiUnavailable,
        DrawReason::NoBaseline,
        DrawReason::SceneChanged,
        DrawReason::UiChanged,
        DrawReason::SkipRunLimit,
    ];

    #[test]
    fn every_reason_has_its_own_counter_and_skips_are_counted_apart() {
        let mut counters = UnchangedFrameCounters::default();
        for (round, reason) in ALL_REASONS.into_iter().enumerate() {
            // Причина номер `round` отмечается `round + 1` раз: перепутанные счётчики дадут разные числа.
            for _ in 0..=round {
                counters.note_drawn(reason);
            }
        }
        counters.note_skipped();
        assert_eq!(counters.skipped, 1, "пропуск не смешивается с отрисовкой");
        assert_eq!(
            [
                counters.drawn_policy_always_redraw,
                counters.drawn_gpu_work_suppressed,
                counters.drawn_new_camera_frame,
                counters.drawn_not_quiet_live_recompose,
                counters.drawn_ui_unavailable,
                counters.drawn_no_baseline,
                counters.drawn_scene_changed,
                counters.drawn_ui_changed,
                counters.drawn_skip_run_limit,
            ],
            [1, 2, 3, 4, 5, 6, 7, 8, 9]
        );
        assert_eq!(counters.drawn_total(), 45);
    }
}
