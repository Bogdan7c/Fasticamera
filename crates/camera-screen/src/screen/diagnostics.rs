//! Диагностика экрана: счётчики, по которым видно, что экран жив и куда уходят события (ничего не глотается молча).

/// Жесты по видам: «распознан» (`Began`/`Ended` по виду) и «отменён» (`Cancelled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GestureCounters {
    pub taps: u64,
    pub long_presses: u64,
    pub pans: u64,
    pub button_presses: u64,
    /// Начатые щипки по видоискателю (зум).
    pub pinches: u64,
    pub cancelled: u64,
}

/// Диагностика экрана.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScreenDiagnostics {
    /// Кадры, по которым выдан список рисования.
    pub frames_produced: u64,
    /// Кадры, по которым источник отказал (рендер при этом показывает видоискатель как обычно).
    pub frame_failures: u64,
    /// Причина последнего отказа кадра.
    pub last_failure: Option<String>,
    /// Сколько раз сцена пересобиралась (первый кадр, смена размера или масштаба).
    pub stage_rebuilds: u64,
    /// Касания, принятые `handle_touch` (образцы).
    pub touches_received: u64,
    /// Касания, пришедшие до первого кадра: сцены ещё нет, они потеряны (счётчик, а не тишина).
    pub touches_dropped_before_first_frame: u64,
    /// Касания (от опускания пальца до отрыва), начатые в зоне обязательного системного жеста: они
    /// принадлежат системе и экраном не обрабатываются.
    pub touches_ignored_in_system_gesture_zone: u64,
    /// Разрывы потока касаний.
    pub touch_gaps: u64,
    pub gestures: GestureCounters,
    /// События жестов, которых не взял ни один виджет (ошибка сборки, должно быть 0).
    pub unhandled_gestures: u64,
    /// Действия: применённые, ничего не изменившие и отказанные редьюсером.
    pub actions_applied: u64,
    pub actions_no_change: u64,
    pub actions_rejected: u64,
    /// Эффекты, попавшие в очередь, и отброшенные из-за переполнения.
    pub effects_queued: u64,
    pub effects_overflowed: u64,
    /// Сколько раз менялся спрос на тёплого соседа (13b): колесо, щипок или долгое нажатие на ленте.
    pub standby_demand_transitions: u64,
    /// События значения EV, погашенные как эхо программной установки (без `SetExposureBias`).
    pub echo_events_suppressed: u64,
    /// Паны «солнца», пришедшие при скрытой рамке.
    pub ignored_sun_pans: u64,
    /// Значения шкал EV, которые не удалось превратить в величину компенсации (действие не выдано).
    pub invalid_exposure_values: u64,
    /// Значения линеек ручек вида, которые не удалось превратить в значение ручки (действие не выдано).
    pub invalid_knob_values: u64,
    /// Имена пресетов, не поместившиеся в ряд таблеток (карточка STYLES или меню раздела): показаны первые
    /// `chip_pool_size`, остальные недоступны для выбора из ряда.
    pub preset_chips_overflowed: u64,
    /// Кадры, пропущенные из-за паузы между пересборками сцены после ошибки.
    pub frames_skipped_rebuild_backoff: u64,
    /// Кадры, где ошибка вида не потребовала пересборки: сцена и идущие жесты сохранены.
    pub frame_failures_scene_kept: u64,
    /// Касания, пришедшие, пока сцены нет после ошибки или смены размера (не «до первого кадра»).
    pub touches_dropped_while_stage_missing: u64,
    /// Жесты, отменённые явным `Cancelled` из-за смены размера или масштаба посреди жеста.
    pub gestures_cancelled_by_stage_change: u64,
    /// Раскладки текста, реально выполненные (промахи кэша); на повторных кадрах не растут.
    pub text_layouts: u64,
    /// Очистки кэша раскладки текста из-за переполнения.
    pub text_cache_evictions: u64,
    /// Миниатюра последнего снимка (14g): сколько картинок показано и что потеряно.
    pub thumbnail: crate::thumbnail::ThumbnailCounters,
}
