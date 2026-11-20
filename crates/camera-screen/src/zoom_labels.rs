//! Подписи зума: правило написания коэффициентов и фокусных (решения 111–112). Чистые функции от чисел
//! каталога камер — ни одной подписи в коде не зашито.
//!
//! # Правила
//! * **Невыбранная** кнопка ленты: до единицы — одна цифра после точки без нуля и без «×» (`.6`); целое — без
//!   десятичной части (`1`, `2`); иначе одна цифра после точки (`1.4`).
//! * **Выбранная** кнопка / значение на колесе: текущее значение жёлтым с «×» (`0.6×`, `1×`, `1.4×`, `10×`);
//!   до 10 — одна цифра после точки, если число не целое; с 10 — целое.
//! * Сначала округление до десятых, потом проверка «целое» (0,96 → `1`, 9,96 → `10×`) — как `format_ev`.
//! * **Фокусное** на колесе: `round(опорное_фокусное × коэффициент)` + «MM» (14MM / 24MM / 47MM).

use camera_contract::zoom::{LensGroup, ZoomRatio};

/// Значение, округлённое до десятых, в десятых долях (`0.593` → 6, `1.04` → 10).
fn tenths(ratio: ZoomRatio) -> i64 {
    (f64::from(ratio.value()) * 10.0).round() as i64
}

/// Подпись невыбранной кнопки ленты: `.6`, `1`, `1.4`, `2`.
pub fn unselected_label(ratio: ZoomRatio) -> String {
    let tenths = tenths(ratio);
    let whole = tenths / 10;
    let decimal = tenths % 10;
    if decimal == 0 {
        format!("{whole}")
    } else if whole == 0 {
        format!(".{decimal}")
    } else {
        format!("{whole}.{decimal}")
    }
}

/// Подпись выбранной кнопки / значения колеса: `0.6×`, `1×`, `1.4×`, `10×`.
pub fn selected_label(ratio: ZoomRatio) -> String {
    let value = f64::from(ratio.value());
    if value >= 10.0 {
        return format!("{}\u{d7}", value.round() as i64);
    }
    let tenths = tenths(ratio);
    let whole = tenths / 10;
    let decimal = tenths % 10;
    if decimal == 0 {
        format!("{whole}\u{d7}")
    } else {
        format!("{whole}.{decimal}\u{d7}")
    }
}

/// Опорное фокусное группы, мм: эквивалентное фокусное объектива, ближайшего к «1×» (у задних — основной).
///
/// Фокусное на коэффициенте `r` — `опорное × r`: кроп основной 2× честно даёт 47–48 MM, а не «24».
pub fn reference_focal_mm(group: &LensGroup) -> Option<f32> {
    group
        .lenses
        .iter()
        .min_by(|left, right| {
            let distance = |ratio: ZoomRatio| (f64::from(ratio.value())).ln().abs();
            distance(left.native_ratio).total_cmp(&distance(right.native_ratio))
        })
        .map(|lens| lens.equivalent_focal_mm / lens.native_ratio.value())
}

/// Подпись фокусного на коэффициенте: `24MM`. `None` — в группе нет объективов.
pub fn focal_label(group: &LensGroup, ratio: ZoomRatio) -> Option<String> {
    let reference = reference_focal_mm(group)?;
    Some(format!("{}MM", (reference * ratio.value()).round() as i64))
}

#[cfg(test)]
#[path = "zoom_labels_tests.rs"]
mod tests;
