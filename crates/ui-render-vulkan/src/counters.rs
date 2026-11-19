//! Счётчики рендера UI для диагностики: сколько кадров записано, отвергнуто, как работала пирамида.

/// Накопленные счётчики. Все только растут.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UiRenderCounters {
    /// Кадров записано успешно.
    pub frames_recorded: u64,
    /// Кадров отвергнуто или упавших (любая ошибка `record_frame`).
    pub frames_failed: u64,
    /// Из них отвергнуто из-за превышения предела кадра.
    pub limit_overflows: u64,
    /// Пирамида перестроена (ключ фона изменился).
    pub pyramid_rebuilds: u64,
    /// Пирамида переиспользована без записи проходов (путь Recompose).
    pub pyramid_reuses: u64,
    /// Наборы ступеней пересозданы под новый логический размер.
    pub pyramid_recreations: u64,
    /// Вызовов рисования в проходе UI.
    pub draw_calls: u64,
    /// Фигур нарисовано.
    pub shape_instances: u64,
    /// Глифов и иконок нарисовано.
    pub glyph_instances: u64,
    /// Групп стекла нарисовано.
    pub glass_groups: u64,
}
