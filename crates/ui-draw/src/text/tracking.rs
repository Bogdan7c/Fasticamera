//! Трекинг SF Pro по кеглю — данные для вызывающего (экран камеры, токены), а не часть вёрстки.
//!
//! Вёрстка ([`super::layout_line`]) трекинг не хардкодит: его передаёт вызывающий в `TextStyle`.
//! Здесь лежит только исходная таблица Apple как стартовая точка подгонки (`docs/ios-camera/assets.md`, 3).

/// `[doc]` HIG Typography, «Tracking values» для SF Pro: (кегль pt, трекинг в 1/1000 em).
/// Для нашего шрифта это **стартовая** точка, подгоняется на глаз («подгонка, не Apple»).
pub const SF_PRO_TRACKING_PER_MILLE: [(f32, f32); 9] = [
    (11.0, 6.0),
    (12.0, 0.0),
    (13.0, -6.0),
    (15.0, -16.0),
    (17.0, -26.0),
    (20.0, -23.0),
    (24.0, 3.0),
    (28.0, 14.0),
    (34.0, 12.0),
];

/// Трекинг SF Pro для кегля в pt, в em. Между строками таблицы — линейная интерполяция `[estimate]`,
/// за краями таблицы — значение крайней строки.
pub fn sf_pro_tracking_em(size_pt: f32) -> f32 {
    let table = &SF_PRO_TRACKING_PER_MILLE;
    let (first_size, first_value) = table[0];
    let (last_size, last_value) = table[table.len() - 1];
    if size_pt <= first_size {
        return first_value / 1000.0;
    }
    if size_pt >= last_size {
        return last_value / 1000.0;
    }
    for pair in table.windows(2) {
        let ((low_size, low_value), (high_size, high_value)) = (pair[0], pair[1]);
        if size_pt <= high_size {
            let fraction = (size_pt - low_size) / (high_size - low_size);
            return (low_value + fraction * (high_value - low_value)) / 1000.0;
        }
    }
    last_value / 1000.0
}
