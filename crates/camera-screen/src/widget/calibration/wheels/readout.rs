//! Числа толчка колеса в строке названия (решение 286): «210° · 35» — угол и сила толчка.
//!
//! Чистая функция от точки толчка, без рисования: подпись пишет `WheelsView::draw_title`.
//! * Угол — направление толчка в градусах 0..360, 0° = ось `+a`, растёт к `+b` (как `atan2(b, a)`; тот же отсчёт, что
//!   у `WheelPush::from_hue_degrees`, то есть показанное число можно подставить обратно и получить ту же точку).
//! * Сила — длина толчка в долях единичного круга, целое 0..100.
//! * Нейтральный толчок направления не имеет: вместо чисел длинное тире (а не «0° · 0», которое выдумало бы угол).

use look_contract::WheelPush;

/// Подпись для нейтрального толчка: направления нет.
const NEUTRAL_READOUT: &str = "—";

/// Подпись толчка для строки названия колеса.
pub(super) fn push_readout(push: WheelPush) -> String {
    if push.is_neutral() {
        return NEUTRAL_READOUT.to_owned();
    }
    format!("{}° · {}", push_angle_degrees(push), push_strength_percent(push))
}

/// Угол толчка, целые градусы 0..=359 (359,6° округляется в 0°, а не в несуществующие 360°).
fn push_angle_degrees(push: WheelPush) -> u32 {
    let degrees = f64::from(push.b()).atan2(f64::from(push.a())).to_degrees();
    let normalized = degrees.rem_euclid(360.0);
    (normalized.round() as u32) % 360
}

/// Сила толчка, целые 0..=100 долей единичного круга.
fn push_strength_percent(push: WheelPush) -> u32 {
    (f64::from(push.strength()) * 100.0).round().clamp(0.0, 100.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(hue_degrees: f32, strength: f32) -> WheelPush {
        WheelPush::from_hue_degrees(hue_degrees, strength).unwrap()
    }

    #[test]
    fn a_known_push_reads_back_the_angle_and_strength_it_was_built_from() {
        assert_eq!(push_readout(push(210.0, 0.35)), "210° · 35");
        assert_eq!(push_readout(push(0.0, 1.0)), "0° · 100");
        assert_eq!(push_readout(push(90.0, 0.5)), "90° · 50");
        assert_eq!(push_readout(push(359.0, 0.07)), "359° · 7");
    }

    #[test]
    fn the_angle_wraps_into_0_to_359_and_never_shows_360() {
        assert_eq!(push_readout(push(359.7, 0.5)), "0° · 50", "359,7° округляется в 0°");
        assert_eq!(push_readout(push(-90.0, 0.5)), "270° · 50", "минус 90° — это 270°");
    }

    #[test]
    fn the_neutral_push_has_no_direction_and_shows_a_dash() {
        assert_eq!(push_readout(WheelPush::NEUTRAL), "—");
    }

    #[test]
    fn a_tiny_push_keeps_its_direction_with_zero_strength() {
        assert_eq!(push_readout(push(45.0, 0.004)), "45° · 0");
    }
}
