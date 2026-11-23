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
    /// Элементов-картинок нарисовано.
    pub images_drawn: u64,
    /// Из них живых картинок с GPU (приборы цвета).
    pub live_images_drawn: u64,
    /// Элементов с живой картинкой, которой на кадре не было (не нарисованы, кадр не упал).
    pub live_images_missing: u64,
    /// Картинок загружено в видеопамять (запись копии в команды кадра).
    pub picture_uploads: u64,
    /// Картинок взято из кэша без загрузки.
    pub picture_cache_hits: u64,
    /// Картинок вытеснено из кэша (память отдана после fence слотов, что их могли читать).
    pub picture_evictions: u64,
    /// Кадров отвергнуто из-за слишком большой картинки.
    pub pictures_rejected: u64,
    /// Картинок выброшено из кэша, потому что кадр с их загрузкой записан, но не отправлен (`frame_not_submitted`).
    pub pictures_discarded_unsubmitted: u64,
}
