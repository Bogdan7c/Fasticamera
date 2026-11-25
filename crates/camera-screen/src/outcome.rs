//! Явный итог каждого действия: применено, ничего не изменилось (и почему) или отказано (и почему).
//!
//! Правило проекта: ошибки и отказы не глотаются молча. Отказ ничего не меняет в состоянии, кроме
//! счётчика [`RejectionCounters`]; эффекты отказанного действия отбрасываются.

/// Итог действия.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionOutcome {
    /// Действие изменило состояние и/или выдало эффекты.
    Applied,
    /// Действие допустимо, но менять нечего.
    NoChange(NoChangeReason),
    /// Действие недопустимо в текущем состоянии. Считается в [`RejectionCounters`].
    Rejected(Rejection),
}

/// Почему ничего не изменилось (это не ошибка).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoChangeReason {
    /// Тап мимо кадра (чёрная полоса): фокус не ставится.
    TapOutsideFrame,
    /// Значение уже такое.
    ValueUnchanged,
    /// Отпускание или автоповтор клавиши: в v1 удержание ничего не значит (место под QuickTake).
    KeyHoldNotSupported,
    /// Долгий тап при открытой панели не блокирует фокус.
    PanelOpen,
    /// Значок над кадром сейчас не показан (например, таймер выключен).
    IconNotShown,
    /// Закрывающий жест при уже закрытых панелях: закрывать нечего.
    NothingToClose,
    /// Фактов о камере нет, показывать нечего (например, точка камеры вне видимого кадра).
    NothingToShow,
    /// Клавиша громкости в режиме, где она не снимает (раздел калибровки, решение 185).
    VolumeKeyIgnoredInMode,
    /// Орган управления уже как в пресете: «↻» тусклая, сбрасывать нечего (решение 196).
    ControlMatchesPreset,
    /// Правок поверх пресета нет: отбрасывать или перезаписывать нечего.
    NoEditsOnTop,
    /// Ответ ввода текста или номер запроса не относится к ожидаемому запросу (устарел, вытеснен, дубль).
    StaleTextResult,
}

/// Причина отказа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// Затвор нажат при открытом меню Controls (решение 94: под шторкой).
    ShutterHiddenByMenu,
    /// Затвор нажат во время съёмки/записи.
    ShutterBusy,
    /// Камера не работает (стартует или недоступна).
    CameraNotLive,
    /// Нет геометрии потока: точку экрана не перевести в сенсор.
    StreamGeometryUnknown,
    /// Не удалось разрешить кадрирование или перевод координат.
    ViewfinderMappingFailed,
    /// Компенсация экспозиции у камеры не поддержана.
    ExposureUnsupported,
    /// Вариант не поддержан (например, автовспышка).
    UnsupportedOption,
    /// Инструмента нет в меню Controls режима.
    ToolNotInMenu,
    /// Инструмента нет среди значков над кадром.
    ToolNotInStrip,
    /// Режим не реализован.
    UnknownMode,
    /// Вариант не относится к открытому подменю или подменю не открыто.
    OptionNotInSubmenu,
    /// Итог или «кадр захвачен» пришёл не для ожидаемого снимка (устарел, дубль или чужой).
    StaleCaptureResult,
    /// Нажатие сверх ёмкости набора сохраняемых снимков (решение 140): «подожди», уже снятое не теряется.
    SavingQueueFull,
    /// Размер surface не даёт раскладки.
    InvalidSurface,
    /// Значок над кадром нажат при открытом меню Controls.
    PanelBlocksAction,
    /// Зум недоступен: каталога камер нет или в нём нет шкалы для этого направления (например, фронталки).
    ZoomUnavailable,
    /// Упора с таким номером в ленте зума нет.
    ZoomStopMissing,
    /// Масштаб щипка или запрошенный коэффициент не число / не больше нуля.
    InvalidZoomInput,
    /// Затвор нажат в режиме без затвора (раздел калибровки, решение 185).
    ShutterAbsentInMode,
    /// Действие раздела калибровки вне этого режима (ручки, приборы, «чистяк», пресеты в меню калибровки).
    NotInCalibrationMode,
    /// Фактов о виде нет: библиотеки пресетов нет, ручки отключены.
    LookFactsMissing,
    /// Текущая основа тона не умеет эту ручку (она показана приглушённой).
    KnobUnsupportedByBase,
    /// Полосы оттенка с таким идентификатором в фактах о виде нет.
    UnknownBand,
    /// Пресета с таким идентификатором в списке пресетов нет (или это не свой пресет там, где нужен свой).
    UnknownPreset,
    /// Активный пресет встроенный: его нельзя перезаписать.
    BuiltinPresetReadOnly,
    /// Действие управления камерой (фокус, экспозиция, зум, значки) в режиме без такого управления
    /// (`ModeBehaviour::camera_controls`).
    NotAvailableInMode,
    /// Кнопки света с таким номером в конфиге нет (или её число не принимается как белое).
    UnknownWhiteLight,
}

impl Rejection {
    /// Число причин (размер таблицы счётчиков). Сверяется с [`Rejection::ALL`] на этапе компиляции.
    const COUNT: usize = 27;

    /// Номер ячейки счётчика. `match` без `_`: новый вариант `Rejection` не скомпилируется, пока ему
    /// не дали номер, — забытая ячейка невозможна. Согласованность номеров с `ALL` проверяет тест.
    const fn slot(self) -> usize {
        match self {
            Rejection::ShutterHiddenByMenu => 0,
            Rejection::ShutterBusy => 1,
            Rejection::CameraNotLive => 2,
            Rejection::StreamGeometryUnknown => 3,
            Rejection::ViewfinderMappingFailed => 4,
            Rejection::ExposureUnsupported => 5,
            Rejection::UnsupportedOption => 6,
            Rejection::ToolNotInMenu => 7,
            Rejection::ToolNotInStrip => 8,
            Rejection::UnknownMode => 9,
            Rejection::OptionNotInSubmenu => 10,
            Rejection::StaleCaptureResult => 11,
            Rejection::InvalidSurface => 12,
            Rejection::PanelBlocksAction => 13,
            Rejection::ZoomUnavailable => 14,
            Rejection::ZoomStopMissing => 15,
            Rejection::InvalidZoomInput => 16,
            Rejection::SavingQueueFull => 17,
            Rejection::ShutterAbsentInMode => 18,
            Rejection::NotInCalibrationMode => 19,
            Rejection::LookFactsMissing => 20,
            Rejection::KnobUnsupportedByBase => 21,
            Rejection::UnknownBand => 22,
            Rejection::UnknownPreset => 23,
            Rejection::BuiltinPresetReadOnly => 24,
            Rejection::NotAvailableInMode => 25,
            Rejection::UnknownWhiteLight => 26,
        }
    }

    /// Все причины (для счётчиков и тестов полноты).
    pub const ALL: [Rejection; Rejection::COUNT] = [
        Rejection::ShutterHiddenByMenu,
        Rejection::ShutterBusy,
        Rejection::CameraNotLive,
        Rejection::StreamGeometryUnknown,
        Rejection::ViewfinderMappingFailed,
        Rejection::ExposureUnsupported,
        Rejection::UnsupportedOption,
        Rejection::ToolNotInMenu,
        Rejection::ToolNotInStrip,
        Rejection::UnknownMode,
        Rejection::OptionNotInSubmenu,
        Rejection::StaleCaptureResult,
        Rejection::InvalidSurface,
        Rejection::PanelBlocksAction,
        Rejection::ZoomUnavailable,
        Rejection::ZoomStopMissing,
        Rejection::InvalidZoomInput,
        Rejection::SavingQueueFull,
        Rejection::ShutterAbsentInMode,
        Rejection::NotInCalibrationMode,
        Rejection::LookFactsMissing,
        Rejection::KnobUnsupportedByBase,
        Rejection::UnknownBand,
        Rejection::UnknownPreset,
        Rejection::BuiltinPresetReadOnly,
        Rejection::NotAvailableInMode,
        Rejection::UnknownWhiteLight,
    ];
}

// Номера ячеек обязаны укладываться в таблицу: проверяется при сборке (полноту `ALL` — тест).
const _: () = {
    let mut index = 0;
    while index < Rejection::ALL.len() {
        assert!(Rejection::ALL[index].slot() < Rejection::COUNT);
        index += 1;
    }
};

/// Счётчики отказов по причинам: отказ не должен пропадать бесследно.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RejectionCounters {
    counts: [u32; Rejection::COUNT],
}

impl RejectionCounters {
    /// Сколько раз отказали по причине.
    pub fn get(&self, reason: Rejection) -> u32 {
        self.counts[reason.slot()]
    }

    /// Сумма по всем причинам.
    pub fn total(&self) -> u32 {
        self.counts.iter().fold(0u32, |sum, count| sum.saturating_add(*count))
    }

    pub(crate) fn record(&mut self, reason: Rejection) {
        let slot = &mut self.counts[reason.slot()];
        *slot = slot.saturating_add(1);
    }
}

#[cfg(test)]
#[path = "outcome_tests.rs"]
mod outcome_tests;
