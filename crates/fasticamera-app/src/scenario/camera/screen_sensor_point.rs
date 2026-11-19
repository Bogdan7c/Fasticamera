//! Независимая проверка «КУДА ушла точка фокуса» для сценария `screen` (сессия 12, ревью P6e).
//!
//! Счётчики шага (тап распознан, намерение ушло, триггер дошёл до HAL) не говорят, в какое место сенсора указала
//! точка: перепутанные оси, зеркало или ориентация сенсора дали бы зелёный. Здесь ожидаемая точка сенсора считается
//! БЕЗ кода экрана (`camera_screen::ScreenLayout` / `ViewfinderMapping` не используются): иначе проверка повторила
//! бы ошибку проверяемого.
//!
//! # Правило
//! Тап задан долями кадра `(u, v)` (`u` вправо, `v` вниз) на экране в портрете. Ориентация сенсора `θ` — на сколько
//! градусов по часовой повернуть кадр сенсора, чтобы он встал прямо. Сенсорная точка `(sx, sy)` при повороте на `θ`
//! попадает на экран так: `0°` — `(sx, sy)`; `90°` — `(1 − sy, sx)`; `180°` — `(1 − sx, 1 − sy)`; `270°` — `(sy, 1 − sx)`.
//! Обратное — ожидаемая точка сенсора ([`expected_sensor_point`]). Для `90°` (OnePlus 11) тап `(0,25; 0,75)` даёт
//! `(0,75; 0,75)`; тот же вывод вручную сделан в `event_router_camera_screen_tests`.
//!
//! # Границы
//! * кадр стартует как 4:3 на ПОЛНЫЙ буфер и полную активную область (так стартует сценарий, `valid_area` = буфер):
//!   кадрирование не сдвигает доли; 16:9 или кроп потребовали бы своего правила;
//! * зеркала нет (задняя камера; фронталки в v1 нет).

use camera_contract::geometry::SensorOrientation;
use camera_contract::three_a::PointOfInterest;

/// Точка внутри прямоугольника кадра на экране: доли `0..=1`, `u` — вправо, `v` — вниз.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramePoint {
    pub u: f32,
    pub v: f32,
}

/// Точка сенсора в долях активной области (`x` вправо, `y` вниз по сенсору).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorFraction {
    pub x: f32,
    pub y: f32,
}

/// Ожидаемая точка сенсора для тапа в точке кадра (правило — в доке модуля).
pub fn expected_sensor_point(frame: FramePoint, orientation: SensorOrientation) -> SensorFraction {
    let FramePoint { u, v } = frame;
    match orientation {
        SensorOrientation::Degrees0 => SensorFraction { x: u, y: v },
        SensorOrientation::Degrees90 => SensorFraction { x: v, y: 1.0 - u },
        SensorOrientation::Degrees180 => SensorFraction { x: 1.0 - u, y: 1.0 - v },
        SensorOrientation::Degrees270 => SensorFraction { x: 1.0 - v, y: u },
    }
}

/// Итог сверки точки интереса движка с ожидаемой.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointCheck {
    /// Точка в пределах допуска от ожидаемой.
    Matches,
    /// У движка нет точки (весь кадр): тап ещё не дошёл.
    NoPoint,
    /// Ориентация сенсора неизвестна (камеры нет в каталоге): судить не из чего.
    UnknownOrientation,
    /// Точка есть, но не там: оси, зеркало или ориентация перепутаны.
    Off {
        expected: SensorFraction,
        actual: SensorFraction,
    },
}

/// Сверяет точку интереса движка с ожидаемой для тапа `frame`; `tolerance` — допуск по каждой оси (доли сенсора).
pub fn check_point(
    point: PointOfInterest,
    orientation: Option<SensorOrientation>,
    frame: FramePoint,
    tolerance: f32,
) -> PointCheck {
    let PointOfInterest::Point { region, .. } = point else {
        return PointCheck::NoPoint;
    };
    let Some(orientation) = orientation else {
        return PointCheck::UnknownOrientation;
    };
    let expected = expected_sensor_point(frame, orientation);
    let actual = SensorFraction {
        x: region.center().x(),
        y: region.center().y(),
    };
    if (actual.x - expected.x).abs() <= tolerance && (actual.y - expected.y).abs() <= tolerance {
        PointCheck::Matches
    } else {
        PointCheck::Off { expected, actual }
    }
}

#[cfg(test)]
#[path = "screen_sensor_point_tests.rs"]
mod tests;
